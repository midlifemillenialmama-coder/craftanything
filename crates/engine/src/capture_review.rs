//! One-use consent for a signed-in website study. No approval is kept across runs.
//! Only local project files are previewed; neither previews nor page contents leave this module
//! as tool results. Digests bind the request to the complete capture, target and saved account.
use crate::tools::{
    site_sign_in::{self, SavedSignIn},
    web_common,
    website_tools::shown_url,
};
use crate::workspace::{TargetRef, Workspace};
use craft_anything_evidence::{ErrorCode, ToolError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

const MAX_FILE: u64 = 32 << 20;
const MAX_CAPTURE: u64 = 512 << 20;
const MAX_FILES: usize = 2048;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Screenshot {
    /// Relative to the project's capture folder; never a URI or an absolute path.
    pub file: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureReview {
    /// Address without query or fragment. This is not an assertion of authentication.
    pub website: String,
    pub capture_sha256: String,
    pub screenshots: Vec<Screenshot>,
}

fn problem(message: &str) -> ToolError {
    ToolError::new(ErrorCode::NoPermission, message)
        .with_remediation("Use web_site_sign_in to sign in again if needed, then start a new study. No captured account content was sent to the builder.")
}

/// Authentication identity chosen once for a website study. A saved profile is a seed, not
/// proof of login. The screenshot approval still checks the captured page's sign-in state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StudyProfile {
    version: u8,
    signed_in: bool,
    signed_in_site: Option<String>,
    sign_in_revision: Option<String>,
}

impl StudyProfile {
    fn from_metadata(metadata: &Value, key: &str) -> Result<Self, ToolError> {
        let signed_in =
            metadata.get("signed_in").and_then(Value::as_bool).ok_or_else(|| problem("The study browser did not record its authentication identity."))?;
        let mut profile = Self { version: 1, signed_in, signed_in_site: None, sign_in_revision: None };
        if signed_in {
            let site = metadata.get("signed_in_site").and_then(Value::as_str);
            let url_key = metadata.get("url").and_then(Value::as_str).and_then(|u| site_sign_in::site_key(u).ok());
            if site != Some(key) || url_key.as_deref() != Some(key) {
                return Err(problem("The study browser belongs to a different website."));
            }
            profile.signed_in_site = Some(key.to_string());
            profile.sign_in_revision = Some(
                metadata
                    .get("sign_in_revision")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| problem("The study browser has no saved-account revision."))?
                    .to_string(),
            );
        }
        Ok(profile)
    }

    /// Read the actual browser identity and endpoint returned by web_site_open, never the
    /// current saved-profile preference. Explicit endpoints prevent browser-tool fallback.
    pub fn opened(url: &str, output: &crate::ToolOutput) -> Result<(Self, String), ToolError> {
        let value = output.to_json();
        let metadata = value.get("result").ok_or_else(|| problem("The study browser did not report its identity."))?;
        let profile = Self::from_metadata(metadata, &site_sign_in::site_key(url)?)?;
        let endpoint = metadata
            .get("cdp_endpoint")
            .and_then(Value::as_str)
            .and_then(|e| craft_anything_engines::cdp::parse_endpoint(e).ok())
            .ok_or_else(|| problem("The study browser did not provide a safe local endpoint."))?;
        Ok((profile, endpoint.origin))
    }

    /// An originally unsigned study stays unsigned even if a sign-in is saved later.
    pub fn open_params(&self, url: &str) -> Value {
        serde_json::json!({"url": url, "signed_in": self.signed_in})
    }

    /// Reopening may start a new browser. Its account revision must match before any capture
    /// or Deep reader can consume its content.
    pub fn reopened(&self, url: &str, output: &crate::ToolOutput) -> Result<String, ToolError> {
        let (profile, endpoint) = Self::opened(url, output)?;
        if &profile != self {
            return Err(problem("The browser's account changed during the study. A fresh study is required."));
        }
        Ok(endpoint)
    }

    pub fn check_captures(&self, ws: &Workspace) -> Result<(), ToolError> {
        let TargetRef::Website { url } = &ws.manifest.target else { return Err(problem("The study's website changed.")) };
        let key = site_sign_in::site_key(url)?;
        let captures = capture_files(&ws.root)?;
        let mut total = 0u64;
        for page in page_folders(&ws.root, &captures)? {
            let bytes = read(&ws.root, &page.join("capture.json"))?;
            charge(&mut total, &bytes)?;
            let metadata: Value = serde_json::from_slice(&bytes).map_err(|_| problem("A captured page's authentication record cannot be read."))?;
            if Self::from_metadata(&metadata, &key)? != *self {
                return Err(problem("The captured pages used different browser identities. A fresh study is required."));
            }
        }
        Ok(())
    }

    /// Complete Deep provenance only after all readers used a verified, explicit endpoint.
    /// Legacy Deep captures without this record cannot establish which account was inspected.
    pub fn record_deep(&self, ws: &Workspace) -> Result<(), ToolError> {
        self.check_captures(ws)?;
        let path = Path::new("capture.json");
        let bytes = read(&ws.root, path)?;
        let mut metadata: Value = serde_json::from_slice(&bytes).map_err(|_| problem("The capture record cannot be read."))?;
        let object = metadata.as_object_mut().ok_or_else(|| problem("The capture record is not an object."))?;
        object.insert("deep_study_profile".into(), serde_json::to_value(self).map_err(|_| problem("The Deep study's identity cannot be recorded."))?);
        let bytes = serde_json::to_vec_pretty(&metadata).map_err(|_| problem("The capture record cannot be written."))?;
        crate::project::write_atomic(&member(&ws.root, path)?, &bytes)
    }
}

/// Resolve a capture member without following links, including a link at capture/ itself.
fn member(root: &Path, relative: &Path) -> Result<PathBuf, ToolError> {
    let mut path = root.join("capture");
    if !std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink()) {
        return Err(problem("The study's capture folder is missing or is a link."));
    }
    for part in relative.components() {
        let Component::Normal(part) = part else { return Err(problem("A screenshot path is outside this project's capture folder.")) };
        path.push(part);
        let meta = std::fs::symlink_metadata(&path).map_err(|_| problem("A captured file is missing or cannot be read."))?;
        if meta.file_type().is_symlink() || (!meta.is_dir() && !meta.is_file()) {
            return Err(problem("A captured file is a link or is not a regular file."));
        }
    }
    Ok(path)
}

fn read(root: &Path, relative: &Path) -> Result<Vec<u8>, ToolError> {
    craft_anything_platform::fs::read_file_capped(&member(root, relative)?, MAX_FILE)
        .map_err(|_| problem("A captured file cannot be read within the preview limits."))
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Inspect the actual tree as well as the route index: failed or unindexed route captures
/// remain visible to a builder even when routes.json does not mention them.
struct CaptureFiles {
    files: Vec<PathBuf>,
    pages: BTreeSet<PathBuf>,
}

fn capture_files(root: &Path) -> Result<CaptureFiles, ToolError> {
    let mut pending = vec![PathBuf::new()];
    let mut files = Vec::new();
    let mut pages = BTreeSet::from([PathBuf::new()]);
    let mut entries = 0usize;
    while let Some(dir) = pending.pop() {
        let path = member(root, &dir)?;
        let children = std::fs::read_dir(path).map_err(|_| problem("The captured files cannot be listed."))?;
        for child in children {
            let child = child.map_err(|_| problem("A captured file cannot be listed."))?;
            entries += 1;
            if entries > MAX_FILES {
                return Err(problem("This capture has too many files to confirm safely."));
            }
            let relative = dir.join(child.file_name());
            let path = member(root, &relative)?;
            if path.is_dir() {
                if dir == Path::new("routes") {
                    pages.insert(relative.clone());
                }
                pending.push(relative);
            } else {
                if relative == Path::new("routes") || dir == Path::new("routes") {
                    return Err(problem("A route capture is not in a page folder with authentication metadata."));
                }
                if relative.file_name().is_some_and(|name| name == "capture.json") {
                    pages.insert(dir.clone());
                }
                files.push(relative);
            }
        }
    }
    files.sort();
    Ok(CaptureFiles { files, pages })
}

/// Portable, project-relative paths only, also on systems where backslash is a normal character.
fn safe_relative(name: &str) -> bool {
    !name.is_empty() && !name.contains(['\\', ':']) && name.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
}

fn portable(path: &Path) -> Result<String, ToolError> {
    let parts: Option<Vec<&str>> = path.iter().map(|p| p.to_str()).collect();
    let name = parts.ok_or_else(|| problem("A capture file name cannot be confirmed safely."))?.join("/");
    if !safe_relative(&name) {
        return Err(problem("A capture path is not a safe local path."));
    }
    Ok(name)
}

fn screenshot_name(name: &str) -> bool {
    safe_relative(name) && !name.contains('/') && name.starts_with("screenshot-") && name.ends_with(".png")
}

fn page_folders(root: &Path, captures: &CaptureFiles) -> Result<BTreeSet<PathBuf>, ToolError> {
    let mut pages = captures.pages.clone();
    if captures.files.iter().any(|p| p == Path::new("routes.json")) {
        let bytes = read(root, Path::new("routes.json"))?;
        let routes: Value = serde_json::from_slice(&bytes).map_err(|_| problem("The route capture index cannot be read."))?;
        let routes = routes.as_array().filter(|r| r.len() <= MAX_FILES).ok_or_else(|| problem("The route capture index is invalid."))?;
        for route in routes {
            let slug = route
                .get("slug")
                .and_then(Value::as_str)
                .filter(|s| safe_relative(s) && !s.contains('/'))
                .ok_or_else(|| problem("A route capture has an unsafe or missing folder name."))?;
            pages.insert(Path::new("routes").join(slug));
        }
    }
    Ok(pages)
}

fn charge(total: &mut u64, bytes: &[u8]) -> Result<(), ToolError> {
    *total = total.saturating_add(bytes.len() as u64);
    if *total > MAX_CAPTURE {
        return Err(problem("This capture is too large to confirm safely."));
    }
    Ok(())
}

/// Capture paths and bytes both contribute to the digest; links and non-files are refused.
fn capture_digest(root: &Path, target: &str, revision: &str, files: &[PathBuf]) -> Result<String, ToolError> {
    let mut hash = Sha256::new();
    hash.update(target.as_bytes());
    hash.update([0]);
    hash.update(revision.as_bytes());
    hash.update([0]);
    let mut total = 0u64;
    for file in files {
        let bytes = read(root, file)?;
        charge(&mut total, &bytes)?;
        let name = portable(file)?;
        hash.update(name.as_bytes());
        hash.update([0]);
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(&bytes);
    }
    Ok(hex::encode(hash.finalize()))
}

/// Prepare the approval at the Study → Build boundary. Saved-profile existence is never taken
/// as proof of login. Old signed-in captures without account identity require a fresh study.
pub fn prepare(ws: &Workspace, saved: Option<&SavedSignIn>) -> Result<Option<CaptureReview>, ToolError> {
    let TargetRef::Website { url } = &ws.manifest.target else { return Ok(None) };
    let key = site_sign_in::site_key(url)?;
    let captures = capture_files(&ws.root)?;
    let pages = page_folders(&ws.root, &captures)?;
    let mut screenshots = Vec::new();
    let mut total = 0u64;
    for page in pages {
        if !page.as_os_str().is_empty() {
            portable(&page)?;
        }
        let bytes = read(&ws.root, &page.join("capture.json"))?;
        charge(&mut total, &bytes)?;
        let metadata: Value = serde_json::from_slice(&bytes).map_err(|_| problem("A captured page's authentication record cannot be read."))?;
        if page.as_os_str().is_empty()
            && (ws.manifest.capture_deep == Some(true) || captures.files.iter().any(|p| p == Path::new("how-it-works.md") || p == Path::new("original.har")))
        {
            let expected = StudyProfile::from_metadata(&metadata, &key)?;
            let recorded: StudyProfile = metadata
                .get("deep_study_profile")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok())
                .ok_or_else(|| problem("This Deep capture has no trustworthy browser identity. Start a fresh study."))?;
            if recorded != expected {
                return Err(problem("The Deep artifacts and captured pages used different browser identities. Start a fresh study."));
            }
        }
        match metadata.get("signed_in").and_then(Value::as_bool) {
            Some(false) => continue,
            Some(true) => {}
            None => return Err(problem("A captured page did not record whether it used a saved sign-in.")),
        }
        let captured_url = metadata.get("url").and_then(Value::as_str).ok_or_else(|| problem("A captured page has no website identity."))?;
        if site_sign_in::site_key(captured_url).ok().as_deref() != Some(key.as_str()) {
            return Err(problem("A captured page belongs to a different website."));
        }
        let saved = saved.ok_or_else(|| problem("The sign-in used by this capture is no longer saved."))?;
        if saved.key != key || metadata.get("signed_in_site").and_then(Value::as_str) != Some(key.as_str()) {
            return Err(problem("A captured page used a sign-in from a different website."));
        }
        if metadata.get("sign_in_revision").and_then(Value::as_str) != Some(saved.revision.as_str()) {
            return Err(problem("The saved account changed, or the captured pages used different accounts."));
        }
        if metadata.get("sign_in_page").and_then(Value::as_bool) != Some(false) {
            return Err(problem("A captured page still shows a sign-in page, or its sign-in state could not be checked."));
        }
        let shots = metadata
            .get("screenshots")
            .and_then(Value::as_array)
            .filter(|s| !s.is_empty() && s.len() <= 4)
            .ok_or_else(|| problem("A signed-in page has no usable first screenshots."))?;
        for shot in shots {
            let file = shot
                .get("file")
                .and_then(Value::as_str)
                .filter(|f| screenshot_name(f))
                .ok_or_else(|| problem("A signed-in page's screenshot has an invalid local file name."))?;
            let relative = page.join(file);
            let bytes = read(&ws.root, &relative)?;
            charge(&mut total, &bytes)?;
            web_common::decode_png(&bytes).map_err(|_| problem("A signed-in page's screenshot cannot be decoded. Please capture the website again."))?;
            screenshots.push(Screenshot { file: portable(&relative)?, sha256: digest(&bytes) });
        }
    }
    if screenshots.is_empty() {
        return Ok(None);
    }
    let saved = saved.ok_or_else(|| problem("The sign-in used by this capture is no longer saved."))?;
    Ok(Some(CaptureReview { website: shown_url(url), capture_sha256: capture_digest(&ws.root, url, &saved.revision, &captures.files)?, screenshots }))
}

impl CaptureReview {
    /// Called on a UI worker. A request cannot make the image loader read outside capture/ or
    /// display content that changed since the request. Nothing is fetched from the website.
    pub fn preview_bytes(&self, root: &Path, screenshot: &Screenshot) -> Result<Vec<u8>, ToolError> {
        if !self.screenshots.contains(screenshot) || !safe_relative(&screenshot.file) || !screenshot.file.rsplit('/').next().is_some_and(screenshot_name) {
            return Err(problem("The screenshot is not part of this confirmation."));
        }
        let bytes = read(root, Path::new(&screenshot.file))?;
        if digest(&bytes) != screenshot.sha256 {
            return Err(problem("The screenshot changed while confirmation was waiting."));
        }
        Ok(bytes)
    }
}
