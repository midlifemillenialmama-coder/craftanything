//! Websites as targets: `web_site_open` starts a private browser owned by the session and loads a
//! page; every other browser tool then looks at that page by default (see
//! [`super::web_common::Browser::resolve`]). `web_site_capture` records what a rebuild needs —
//! full-page screenshots at several widths, the rendered HTML, design tokens, a layout outline,
//! assets, links and form outlines — into a folder. `web_site_close` ends the browser.
//!
//! Data minimisation: form field values, cookies and storage are never read; the capture holds
//! what any visitor sees on the page.

use super::artifact_tools::{create_absent_dir, write_new_file};
use super::browser_tools::{browser_env, cleanup, deadline};
use super::native_common::spec;
use super::site_sign_in;
use super::web_common::{self, Browser, LaunchOptions, ProfileChoice};
use crate::tool::{Caller, Ctx, Effects, Requires, ToolOutput, ToolSpec, Toolset, parse_params, schema_of};
use craft_anything_engines::cdp::{self, CdpClient, CdpEvent, Deadline, DialogAnswer, Endpoint, PAGE_MAX_MESSAGE};
use craft_anything_evidence::{
    Authority, Confidence, Coverage, CoverageStatus, ErrorCode, EvidenceBuilder, Limitation, Location, Subject, ToolError, sha256_hex,
};
use craft_anything_platform::sync::Mutex;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

pub fn specs() -> Vec<ToolSpec> {
    vec![
        spec(
            "web_site_open",
            "Open a website",
            "Load a web page (http/https) in a private browser owned by this session (fresh profile, no history or logins) at a chosen window size, wait for it to finish loading, and report its final address, title and size. When web_site_sign_in saved a sign-in for that exact site, the private profile starts with a copy of it (signed_in: false opens signed out; true requires one). Every browser tool then inspects this page unless given another endpoint. Runs the site's scripts, so it needs your approval.",
            "Open a website in a private, hidden browser so it can be studied.",
            Toolset::Browser,
            Effects::EVIDENCE
                .union(Effects::EXECUTES_FOREIGN_CODE)
                .union(Effects::ACCESSES_NETWORK)
                .union(Effects::LAUNCHES_PROCESS)
                .union(Effects::WRITES_FILESYSTEM),
            Requires::Engine("chromium"),
            schema_of::<OpenParams>,
            web_site_open,
        ),
        spec(
            "web_site_capture",
            "Capture a website for rebuilding",
            "Record the open website into a new folder: full-page PNG screenshots at each requested width, the rendered HTML, design tokens (fonts, sizes, weights, text and background colours as hex, radii, shadows, spacing, content widths, CSS custom properties), the widths its CSS switches layout at, the layout at each width (page width, heading and text sizes, each section's padding, gap, content column and items per row), a layout outline of visible landmarks, headings, controls and images with positions, asset lists (images, stylesheets, scripts, font files, the requests it made), links and form outlines (never values). Writes capture.json and a README.md summary.",
            "Save pictures and a blueprint of a web page so it can be rebuilt.",
            Toolset::Browser,
            Effects::EVIDENCE.union(Effects::WRITES_FILESYSTEM).union(Effects::ACCESSES_NETWORK),
            Requires::Engine("chromium"),
            schema_of::<CaptureParams>,
            web_site_capture,
        ),
        spec(
            "web_page_states_capture",
            "Capture menus, dialogs and tabs",
            "On the open website, open up to max_states of its visible menus, pop-ups, tabs and collapsed sections (aria-expanded, aria-haspopup, unselected tabs, <summary>; never form controls or submit buttons) one at a time: click it, take a screenshot, record what changed (expanded, a dialog appeared, the tab or the address changed), then put the page back. Writes state-<n>.png and states.json into a new folder. Clicking runs the site's own code, so it needs your approval.",
            "Open the site's menus, pop-ups and tabs and photograph each, so a rebuild can match them too.",
            Toolset::Browser,
            Effects::EVIDENCE.union(Effects::WRITES_FILESYSTEM).union(Effects::EXECUTES_FOREIGN_CODE).union(Effects::ACCESSES_NETWORK),
            Requires::Engine("chromium"),
            schema_of::<StatesParams>,
            web_page_states_capture,
        ),
        spec(
            DIALOG_ANSWER_TOOL,
            "Answer a page's question",
            "Answer the confirm or prompt the open website is waiting on (web_site_open and web_site_capture report one as dialog_open; nothing in the page runs until it's answered): accept: true agrees (OK), false dismisses (Cancel); prompt_text is typed into a prompt before it's accepted; the parameter itself isn't kept in the evidence (only whether one was given), but what the page shows or logs afterwards (its title, a console error) can repeat it. Then waits for the page to settle and reports where it ended up, including the next question if it asks one. Agreeing lets the site do what it asked, so it needs your approval.",
            "Click OK or Cancel on a question the website is asking.",
            Toolset::Browser,
            Effects::EVIDENCE.union(Effects::EXECUTES_FOREIGN_CODE).union(Effects::ACCESSES_NETWORK).union(Effects::MUTATES_SESSION),
            Requires::Engine("chromium"),
            schema_of::<DialogAnswerParams>,
            web_page_dialog_answer,
        ),
        spec(
            "web_site_close",
            "Close the website",
            "Close the private browser started by web_site_open and delete its temporary profile.",
            "Close the hidden browser.",
            Toolset::Browser,
            Effects::MUTATES_SESSION.union(Effects::IDEMPOTENT),
            Requires::Engine("chromium"),
            schema_of::<CloseParams>,
            web_site_close,
        ),
    ]
}

/// The session's website browser. The process lives as long as this slot (it is killed and its
/// profile removed when the last handle drops).
pub struct SiteBrowser {
    pub endpoint: Endpoint,
    pub product: String,
    pub version: String,
    pub url: Mutex<String>,
    /// What went wrong while the last page opened ([`page_health`]); web_site_capture records it.
    pub health: Mutex<Value>,
    /// The `confirm` or `prompt` the page is waiting on, with the connection that saw it open:
    /// set by the tool that saw it, taken by web_page_dialog_answer or the next web_site_open.
    held: Mutex<Option<HeldDialog>>,
    /// The site key whose saved sign-in this browser started with; `None`: a fresh private profile.
    pub signed_in: Option<String>,
    /// Revision of the saved profile copied at launch; `None` for a fresh profile.
    pub sign_in_revision: Option<String>,
    _owner: Mutex<Browser>,
}

impl SiteBrowser {
    /// A non-owning handle for the shared browser helpers.
    fn handle(&self) -> Browser {
        Browser::attached(&self.endpoint, &self.product, &self.version)
    }
}

// ---------------------------------------------------------------------------------------------
// web_site_open

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenParams {
    /// The page to open (http or https).
    pub url: String,
    /// Window width in CSS pixels (320..=3840, default 1440).
    #[serde(default = "d_width")]
    pub width: u32,
    /// Window height in CSS pixels (320..=4000, default 900).
    #[serde(default = "d_height")]
    pub height: u32,
    /// Extra time to let late content settle after the load event, in milliseconds (default 2000).
    #[serde(default = "d_settle")]
    pub settle_ms: u64,
    /// Run the browser without a window (default true).
    #[serde(default = "yes")]
    pub headless: bool,
    #[serde(default = "d_timeout")]
    pub timeout_secs: u64,
    /// Use the sign-in saved for this site by web_site_sign_in. Unset: use it when one is saved;
    /// false: a fresh private profile; true: it must be saved (an error otherwise).
    #[serde(default)]
    pub signed_in: Option<bool>,
}

fn d_width() -> u32 {
    1440
}
fn d_height() -> u32 {
    900
}
fn d_settle() -> u64 {
    2000
}
fn d_timeout() -> u64 {
    90
}
fn yes() -> bool {
    true
}

/// Only http(s) pages; never local files or browser-internal schemes.
pub fn check_url(url: &str) -> Result<&str, ToolError> {
    let u = url.trim();
    let rest = u.strip_prefix("https://").or_else(|| u.strip_prefix("http://"));
    match rest {
        Some(r) if !r.is_empty() && !r.starts_with('/') && !u.chars().any(char::is_whitespace) => Ok(u),
        _ => Err(ToolError::invalid(format!("{url:?} is not a web address")).with_remediation("Give a full address such as https://example.com.")),
    }
}

/// A URL as shown in evidence and summaries: no credentials, query or fragment.
pub(crate) fn shown_url(url: &str) -> String {
    web_common::sanitize_url(url).split(['?', '#']).next().unwrap_or_default().to_string()
}

/// Open a page, reusing a live browser only when its requested profile still matches.
fn web_site_open(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: OpenParams = parse_params(params)?;
    let url = check_url(&p.url)?.to_string();
    if !(320..=3840).contains(&p.width) || !(320..=4000).contains(&p.height) {
        return Err(ToolError::invalid("width must be 320..=3840 and height 320..=4000"));
    }
    let dl = deadline(ctx, p.timeout_secs)?;
    let saved = chosen_sign_in(ctx.session, &url, p.signed_in)?;
    let reservation = saved.as_ref().map(|s| site_sign_in::lock_site(ctx.session, &s.key)).transpose()?;
    // Re-read under the reservation: a different engine may have replaced or forgotten it.
    let saved = if reservation.is_some() { chosen_sign_in(ctx.session, &url, p.signed_in)? } else { saved };
    let want = saved.as_ref().map(|s| s.key.clone());
    // A browser started for another sign-in (or none) isn't reused: each site sees only its own.
    // The new one replaces it once it has started, so a failed start leaves the old one open.
    let revision = saved.as_ref().map(|s| s.revision.clone());
    let reuse = live_site(ctx, &dl).filter(|s| reuse_profile(p.signed_in, s.signed_in.as_deref(), s.sign_in_revision.as_deref(), saved.as_ref()));
    let site = match reuse {
        Some(s) => s,
        None => {
            ctx.report(None, if want.is_some() { "Starting a private browser with your saved sign-in…" } else { "Starting a private browser…" });
            let profile = saved.as_ref().map_or(ProfileChoice::Fresh, |s| ProfileChoice::SeededFrom(s.profile.clone()));
            let opts = LaunchOptions { headless: p.headless, profile, ..LaunchOptions::default() };
            let b = Browser::launch(ctx, &opts, &dl)?;
            let site = Arc::new(SiteBrowser {
                endpoint: b.endpoint.clone(),
                product: b.product.clone(),
                version: b.version.clone(),
                url: Mutex::new(String::new()),
                health: Mutex::new(Value::Null),
                held: Mutex::new(None),
                signed_in: want.clone(),
                sign_in_revision: revision,
                _owner: Mutex::new(b),
            });
            ctx.session.ext.insert(site.clone());
            site
        }
    };
    drop(reservation);
    ctx.report(None, format!("Loading {url}…"));
    // Opening another page is leaving a question unanswered, as a visitor typing an address does.
    release_held(&site);
    let mut client = page_client(&site, &dl)?;
    // A page waiting on a question can't be read: its facts wait for the answer.
    let r = navigate(&mut client, &url, p.width, p.height, p.settle_ms, &dl).and_then(|health| facts_unless_asked(&mut client, health, &dl));
    let waiting = client.waiting_dialog().map(dialog_open);
    // The page that asked (the opening's frame address): with no facts read, that's where the
    // page is, which may still be the previous page if it asked before the navigation took.
    let asked_at = client.waiting_dialog().and_then(|d| d.get("url")).and_then(Value::as_str).filter(|u| !u.is_empty()).map(str::to_string);
    match &waiting {
        Some(open) => hold(&site, open.clone(), client),
        None => cleanup(&mut client, &dl),
    }
    let (facts, health) = r?;
    let facts = facts.unwrap_or(Value::Null);
    let final_url = page_at(&facts, asked_at, &url);
    if let Ok(mut u) = site.url.lock() {
        u.clone_from(&final_url);
    }
    if let Ok(mut h) = site.health.lock() {
        h.clone_from(&health);
    }
    // Clean room keeps what a visitor sees: not the page's load errors, failed requests or size in
    // elements. A dialog the page opened is something a visitor sees, so it's said either way.
    let dialogs = health.get("dialogs").cloned().unwrap_or_else(|| json!([]));
    let (visitor, mut facts) = (ctx.session.clean_room(), facts);
    let health = if visitor {
        if let Some(f) = facts.as_object_mut() {
            f.remove("elements");
        }
        Value::Null
    } else {
        health
    };
    let title = facts.get("title").and_then(Value::as_str).unwrap_or_default().to_string();
    let shown = shown_url(&final_url);
    let how = if want.is_some() { " signed in" } else { "" };
    let opened = opened_said(&shown_url(&url), &shown, how, &title, waiting.is_some());
    let count = |k: &str| health.get(k).and_then(Value::as_array).map_or(0, Vec::len);
    let summary = match (count("errors"), count("failed_requests")) {
        (0, 0) => opened,
        (e, f) => format!("{opened} While loading: {e} error(s), {f} failed request(s)."),
    };
    let summary = format!("{summary}{}{}", dialogs_said(&dialogs), waiting.as_ref().map(dialog_waiting_said).unwrap_or_default());
    // Pages opened signed in often land on addresses carrying codes or tokens: keep only the path.
    let (final_url, facts) = if want.is_some() { signed_in_urls(final_url, facts) } else { (final_url, facts) };
    let browser = site.handle();
    let mut ev = EvidenceBuilder::new(
        "web_site_open",
        Subject { kind: "website".into(), sha256: None, name: Some(shown.clone()) },
        browser_env(ctx.session.clean_room(), &browser),
    )
    .params(json!({"url": shown, "width": p.width, "height": p.height}))
    .result(json!({"url": final_url, "title": title, "facts": facts, "health": health, "dialogs": dialogs, "dialog_open": waiting, "browser": site.product, "signed_in": want.is_some(), "signed_in_site": want}))
    .summary(summary)
    .confidence(Confidence::Observed)
    .authority(Authority::OurRun)
    .limitation(profile_limitation(want.as_deref()))
    .location(Location::new("url", shown));
    ev = match &waiting {
        Some(_) => {
            ev.limitation(waiting_limitation()).coverage(Coverage { status: CoverageStatus::Partial, examined: Some(0), total: Some(1), next_offset: None })
        }
        None => ev.coverage(Coverage::complete(1)),
    };
    Ok(ToolOutput::evidence(ev.build()?))
}

/// The page's facts after [`navigate`] (none while it waits on a question), with its health. A
/// question asked while the facts are read stops the read there: it's reported as one asked while
/// the page loaded, with what the page said before it. Other read failures are returned.
fn facts_unless_asked(client: &mut CdpClient, health: Value, dl: &Deadline) -> Result<(Option<Value>, Value), ToolError> {
    if health.get("dialog_open").is_some() {
        return Ok((None, health));
    }
    match page_facts(client, dl) {
        Ok(facts) => Ok((Some(facts), health)),
        Err(_) if client.waiting_dialog().is_some() => {
            let mut seen = Vec::new();
            keep_health_events(&mut seen, client.take_events());
            Ok((None, merge_health(health, &health_with_dialog(client, &seen))))
        }
        Err(e) => Err(e),
    }
}

/// Where the page is after [`navigate`]: what its facts read, else the address of the frame that
/// asked the question it's waiting on (with nothing read, the page that asked may still be the
/// previous one), else the address requested.
fn page_at(facts: &Value, asked_at: Option<String>, requested: &str) -> String {
    facts.get("url").and_then(Value::as_str).map(str::to_string).or(asked_at).unwrap_or_else(|| requested.to_string())
}

/// The summary's first sentence for `web_site_open`: "Opened" only when the page is the one
/// requested, not when another page's question stopped it first.
fn opened_said(requested: &str, shown: &str, how: &str, title: &str, waiting: bool) -> String {
    if waiting && shown != requested {
        // A redirect or the previous page: which one, nothing read here can tell.
        format!("Went to {requested}{how}; the page at {shown} asked a question before it could be read.")
    } else if title.is_empty() {
        format!("Opened {shown}{how}.")
    } else {
        format!("Opened {shown}{how}: \"{title}\".")
    }
}

/// A question the page is waiting on, kept with the connection that saw it open. Only that
/// connection can answer it: Chrome tells a new one there's no dialog, and doesn't answer its
/// `Page.enable` while the question is open.
struct HeldDialog {
    /// [`dialog_open`]: what it asks.
    open: Value,
    client: CdpClient,
}

/// The question the open website's page is waiting on, if any.
fn held_dialog(site: &SiteBrowser) -> Option<Value> {
    site.held.lock().ok().and_then(|h| h.as_ref().map(|h| h.open.clone()))
}

/// Keep `client` until the question it saw is answered.
fn hold(site: &SiteBrowser, open: Value, client: CdpClient) {
    let old = site.held.lock().ok().and_then(|mut h| h.replace(HeldDialog { open, client }));
    if let Some(old) = old {
        dismiss(old);
    }
}

fn take_held(site: &SiteBrowser) -> Option<HeldDialog> {
    site.held.lock().ok().and_then(|mut h| h.take())
}

/// Dismiss a held question, as a visitor who leaves the page does.
fn release_held(site: &SiteBrowser) {
    if let Some(h) = take_held(site) {
        dismiss(h);
    }
}

/// Answer "no" and close the connection: one closed with the question still open would leave the
/// page stuck, with nothing left that can answer it.
fn dismiss(mut held: HeldDialog) {
    let quick = Deadline::new(Duration::from_secs(5), crate::tool::CancelToken::default());
    // Cancel can make the page ask something else; that's dismissed as well while connected.
    held.client.dismiss_from_now();
    let _ = held.client.call("Page.handleJavaScriptDialog", json!({"accept": false}), None, &quick);
    cleanup(&mut held.client, &quick);
}

/// What a page's `confirm` or `prompt` asks, as kept and shown: its kind, first line and default
/// answer (a prompt's), never anything typed into it.
pub(crate) fn dialog_open(opening: &Value) -> Value {
    let line = opening.get("message").and_then(Value::as_str).unwrap_or_default().lines().next().unwrap_or_default().chars().take(300).collect::<String>();
    let mut d = json!({"type": opening["type"], "message": line});
    if opening["type"] == "prompt"
        && let Some(o) = d.as_object_mut()
    {
        o.insert(
            "default_prompt".into(),
            json!(opening.get("defaultPrompt").and_then(Value::as_str).unwrap_or_default().chars().take(300).collect::<String>()),
        );
    }
    d
}

/// The summary's sentence on a question the page is waiting on.
fn dialog_waiting_said(open: &Value) -> String {
    let kind = if open["type"] == "prompt" { "a prompt" } else { "a confirm question" };
    format!(
        " The page is waiting for an answer to {kind} (\"{}\"): nothing in it can be read until it's answered. Answer it with {DIALOG_ANSWER_TOOL} (accept: true agrees, false dismisses).",
        open["message"].as_str().unwrap_or_default()
    )
}

fn waiting_limitation() -> Limitation {
    Limitation::new(
        "dialog_waiting",
        format!("The page is waiting on a question it asked; its title, size and contents weren't read. Answer it with {DIALOG_ANSWER_TOOL}."),
    )
}

/// Finish with a tool's client: when the page stopped it with a question, keep the client for the
/// answer and say what the page asks and how to answer it; otherwise close it and pass `e` on.
fn end_with(site: &SiteBrowser, mut client: CdpClient, e: ToolError, dl: &Deadline) -> ToolError {
    match client.waiting_dialog().map(dialog_open) {
        Some(open) => {
            hold(site, open.clone(), client);
            dialog_refusal(&open)
        }
        None => {
            cleanup(&mut client, dl);
            e
        }
    }
}

/// A capture a question stopped part-way: its folder already holds what was captured before it,
/// and a capture never writes into an existing folder, so the retry needs a new one.
fn capture_interrupted(e: ToolError, output_directory: &str) -> ToolError {
    if e.details.as_ref().is_none_or(|d| d.get(cdp::DIALOG_DETAIL).is_none()) {
        return e;
    }
    e.with_remediation(format!(
        "Answer it with {DIALOG_ANSWER_TOOL} (accept: true agrees, false dismisses), then run web_site_capture again with a new output_directory: {output_directory} keeps what was captured before the question."
    ))
}

fn dialog_refusal(open: &Value) -> ToolError {
    ToolError::new(
        ErrorCode::ProgramFailed,
        format!(
            "The page is waiting for an answer to its {} (\"{}\"), so nothing in it can be read.",
            open["type"].as_str().unwrap_or("dialog"),
            open["message"].as_str().unwrap_or_default()
        ),
    )
    .with_remediation(format!("Answer it with {DIALOG_ANSWER_TOOL} (accept: true agrees, false dismisses), then run this again."))
    .with_details(json!({ cdp::DIALOG_DETAIL: open }))
}

/// Refuse to start on a page that is waiting on a question.
pub(crate) fn refuse_if_waiting(site: &SiteBrowser) -> Result<(), ToolError> {
    held_dialog(site).map_or(Ok(()), |open| Err(dialog_refusal(&open)))
}

/// The summary's sentence on dialogs the page opened by itself while it loaded (empty when none).
fn dialogs_said(dialogs: &Value) -> String {
    // The one still waiting has its own sentence (dialog_waiting_said).
    let answered: Vec<Value> = dialogs.as_array().into_iter().flatten().filter(|d| d["answered"] != "waiting").cloned().collect();
    web_common::dialogs_said(&answered)
}

/// Explicit signed-out opens must discard even a matching browser's accumulated state.
fn reuse_profile(requested: Option<bool>, key: Option<&str>, revision: Option<&str>, saved: Option<&site_sign_in::SavedSignIn>) -> bool {
    requested != Some(false) && key == saved.map(|s| s.key.as_str()) && revision == saved.map(|s| s.revision.as_str())
}

/// The saved sign-in `web_site_open` starts with for `url`: the one saved for its exact site,
/// unless `signed_in` is false. `true` with none saved is an error naming the tool that saves one.
fn chosen_sign_in(session: &crate::session::Session, url: &str, signed_in: Option<bool>) -> Result<Option<site_sign_in::SavedSignIn>, ToolError> {
    if signed_in == Some(false) {
        return Ok(None);
    }
    let key = site_sign_in::site_key(url);
    let saved = key.as_ref().ok().and_then(|k| site_sign_in::saved(session, k));
    if signed_in == Some(true) && saved.is_none() {
        let site = key.unwrap_or_else(|_| shown_url(url));
        return Err(ToolError::new(ErrorCode::NotFound, format!("No sign-in is saved for {site}.")).with_remediation(format!(
            "Sign in once with {} (it opens a window where you sign in yourself), then open the site again.",
            site_sign_in::SIGN_IN_TOOL
        )));
    }
    Ok(saved)
}

/// A signed-in page's address and the facts' copy of it without query or fragment.
fn signed_in_urls(url: String, mut facts: Value) -> (String, Value) {
    if let Some(f) = facts.as_object_mut()
        && let Some(u) = f.get("url").and_then(Value::as_str).map(shown_url)
    {
        f.insert("url".into(), json!(u));
    }
    (shown_url(&url), facts)
}

/// What the opened pages' browser profile means for what they show.
fn profile_limitation(signed_in: Option<&str>) -> Limitation {
    match signed_in {
        Some(site) => Limitation::new(
            "signed_in",
            format!(
                "Opened signed in with your saved sign-in for {site}: screenshots and page text may show your own account's data. If the site has ended that sign-in since, pages show their signed-out state."
            ),
        ),
        None => Limitation::new("private_profile", "A fresh private browser: pages behind a login show their signed-out state."),
    }
}

/// The session's website browser when it is still answering; a dead one is dropped.
fn live_site(ctx: &mut Ctx<'_>, dl: &Deadline) -> Option<Arc<SiteBrowser>> {
    let site = ctx.session.ext.get::<SiteBrowser>()?;
    if cdp::json_version(&site.endpoint, dl).is_ok() {
        return Some(site);
    }
    ctx.session.ext.remove::<SiteBrowser>();
    None
}

/// The site `web_site_open` opened in this session. Each CLI command is a session of its own, and
/// its browser closes when the command ends, so from the CLI the steps belong in one session (#153).
fn open_site(ctx: &mut Ctx<'_>, dl: &Deadline) -> Result<Arc<SiteBrowser>, ToolError> {
    live_site(ctx, dl).ok_or_else(|| {
        let fix = match ctx.caller {
            Caller::Cli => {
                "Each CLI command starts its own browser and closes it when it ends, so a site opened by an earlier command isn't open here. Run web_site_open and the tools that use the page in one session: an MCP client (`craft-anything-cli mcp`), the app, or `craft-anything-cli rebuild`."
            }
            _ => "Open one first with web_site_open.",
        };
        ToolError::new(ErrorCode::NotFound, "no website is open").with_remediation(fix)
    })
}

/// A client for the open website's page. Nobody sits at this private browser, so a page's own
/// `alert` would hold it until the deadline: notices are answered at once. A `confirm` or `prompt`
/// is a real question, so it's held for the agent (web_page_dialog_answer).
fn page_client(site: &SiteBrowser, dl: &Deadline) -> Result<CdpClient, ToolError> {
    connect_page(site, dl).map(CdpClient::answering_notices)
}

/// A client that also dismisses a `confirm` or `prompt`: for clicks of our own
/// (web_page_states_capture), which never agree to what a control asks.
fn page_client_dismissing(site: &SiteBrowser, dl: &Deadline) -> Result<CdpClient, ToolError> {
    connect_page(site, dl).map(CdpClient::answering_dialogs)
}

fn connect_page(site: &SiteBrowser, dl: &Deadline) -> Result<CdpClient, ToolError> {
    let b = site.handle();
    let targets = cdp::json_list(&site.endpoint, dl)?;
    let target = web_common::pick_target(&targets, None, Some("page"))?;
    web_common::connect_target(&b, target, PAGE_MAX_MESSAGE, dl)
}

/// Size the page and pin light mode, so captures don't depend on this computer's theme.
fn set_viewport(client: &mut CdpClient, width: u32, height: u32, dl: &Deadline) -> Result<(), ToolError> {
    client.call("Emulation.setDeviceMetricsOverride", json!({"width": width, "height": height, "deviceScaleFactor": 1, "mobile": width < 600}), None, dl)?;
    let _ = client.call("Emulation.setEmulatedMedia", json!({"features": [{"name": "prefers-color-scheme", "value": "light"}]}), None, dl);
    Ok(())
}

/// Open `url` and wait for it to settle; returns what went wrong while it loaded ([`page_health`]).
/// A page that stops at a `confirm` or `prompt` the client holds ([`CdpClient::answering_notices`])
/// ends the wait at once, with the question as `dialog_open` ([`dialog_open`]).
pub(crate) fn navigate(client: &mut CdpClient, url: &str, width: u32, height: u32, settle_ms: u64, dl: &Deadline) -> Result<Value, ToolError> {
    client.call("Page.enable", json!({}), None, dl)?;
    // Errors and failed requests are read from the browser's own events; a browser that refuses
    // one of these still loads the page.
    for domain in ["Runtime.enable", "Log.enable", "Network.enable"] {
        let _ = client.call(domain, json!({}), None, dl);
    }
    match set_viewport(client, width, height, dl) {
        // The previous page asked something as it was resized: answered below, then navigated.
        Err(_) if client.waiting_dialog().is_some() => {}
        r => r?,
    }
    let mut seen: Vec<CdpEvent> = Vec::new();
    // What came before is the previous page's (its own load finishing, its errors): only this
    // navigation's events are read, so an earlier load event can't end the wait early.
    let _ = client.take_events();
    // A question the previous page asked during setup is left unanswered, as a visitor typing an
    // address does; otherwise the navigation would never be sent.
    if client.waiting_dialog().is_some() {
        client.call("Page.handleJavaScriptDialog", json!({"accept": false}), None, dl)?;
    }
    let r = match client.call("Page.navigate", json!({"url": url}), None, dl) {
        // The question came before the browser said the navigation started.
        Err(_) if client.waiting_dialog().is_some() => Value::Null,
        r => r?,
    };
    if let Some(err) = r.get("errorText").and_then(Value::as_str).filter(|s| !s.is_empty()) {
        return Err(ToolError::new(ErrorCode::ProgramFailed, format!("the page didn't load: {err}"))
            .with_remediation("Check the address and your internet connection, then try again."));
    }
    // A navigation to a new document names its loader: only that document's load ends the wait.
    let loader = r.get("loaderId").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
    wait_for_page(client, settle_ms, loader.as_deref(), &mut seen, dl)?;
    for domain in ["Network.disable", "Log.disable"] {
        let _ = client.call(domain, json!({}), None, dl);
    }
    Ok(health_with_dialog(client, &seen))
}

/// Wait for the load event (at most 45 s, bounded by the overall deadline), then let late content
/// settle, keeping the events [`page_health`] reads. A question the page asks ends the wait.
/// With `loader` (the `loaderId` `Page.navigate` returned), a load event counts only after the
/// main frame committed that document (`Page.frameNavigated`), so the previous page finishing its
/// own load while the navigation was sent can't end the wait.
fn wait_for_page(client: &mut CdpClient, settle_ms: u64, loader: Option<&str>, seen: &mut Vec<CdpEvent>, dl: &Deadline) -> Result<(), ToolError> {
    let loaded = Deadline::new(Duration::from_secs(45), crate::tool::CancelToken::default());
    let mut committed = loader.is_none();
    // In order: a load event before this document committed is the previous page's.
    let mut finished = |events: &[CdpEvent]| {
        let mut done = false;
        for e in events {
            match e.method.as_str() {
                "Page.frameNavigated" if e.params["frame"].get("parentId").is_none() && loader.is_some_and(|l| e.params["frame"]["loaderId"] == l) => {
                    committed = true;
                }
                "Page.loadEventFired" if committed => done = true,
                _ => {}
            }
        }
        done
    };
    // A load that finished while Page.navigate was answered is in the queue already.
    let queued = client.take_events();
    let mut done = finished(&queued);
    keep_health_events(seen, queued);
    while !done && client.waiting_dialog().is_none() && !loaded.expired() && !dl.expired() {
        let slice = Deadline::new(Duration::from_millis(250), crate::tool::CancelToken::default());
        client.pump_until(&slice)?;
        let events = client.take_events();
        done = finished(&events);
        keep_health_events(seen, events);
    }
    // In slices, so a question asked while the page settles ends the wait too.
    let settled = std::time::Instant::now() + Duration::from_millis(settle_ms.min(30_000));
    loop {
        let left = settled.saturating_duration_since(std::time::Instant::now());
        if client.waiting_dialog().is_some() || left.is_zero() || dl.expired() {
            break;
        }
        client.pump_until(&Deadline::new(left.min(Duration::from_millis(250)), crate::tool::CancelToken::default()))?;
        keep_health_events(seen, client.take_events());
    }
    Ok(())
}

/// The stored health of a page whose held question was just answered (`accept`): that question's
/// `waiting` becomes the answer given, and what the page reported after it (`after`, a
/// [`page_health`]) is added, with the question it asks next, if any, as `dialog_open`.
fn health_after_answer(stored: &Value, accept: bool, after: &Value) -> Value {
    let mut health = match stored {
        Value::Object(_) => stored.clone(),
        _ => json!({"errors": [], "failed_requests": []}),
    };
    if let Some(d) = health.get_mut("dialogs").and_then(Value::as_array_mut).and_then(|ds| ds.iter_mut().rev().find(|d| d["answered"] == "waiting")) {
        d["answered"] = json!(if accept { "accepted" } else { "dismissed" });
    }
    merge_health(health, after)
}

/// `health` with what the page reported later (`after`, a [`page_health`]) added, capped as
/// [`page_health`] caps it, and `after`'s question (or none) as `dialog_open`.
fn merge_health(mut health: Value, after: &Value) -> Value {
    let Some(o) = health.as_object_mut() else { return health };
    for (key, cap) in [("errors", 30), ("failed_requests", 50), ("dialogs", 30)] {
        let Some(new) = after.get(key).and_then(Value::as_array) else { continue };
        let list = o.entry(key).or_insert_with(|| json!([]));
        if let Some(list) = list.as_array_mut() {
            for item in new {
                // An error is kept once per text, as page_health does.
                let seen = key == "errors" && list.iter().any(|e| e["text"] == item["text"]);
                if !seen && list.len() < cap {
                    list.push(item.clone());
                }
            }
        }
    }
    match after.get("dialog_open") {
        Some(open) => o.insert("dialog_open".into(), open.clone()),
        None => o.remove("dialog_open"),
    };
    health
}

/// [`page_health`] for what this client saw, with the question it holds as `dialog_open`.
fn health_with_dialog(client: &CdpClient, seen: &[CdpEvent]) -> Value {
    let mut health = page_health(seen, !client.dismisses_questions());
    if let (Some(open), Some(o)) = (client.waiting_dialog(), health.as_object_mut()) {
        o.insert("dialog_open".into(), dialog_open(open));
    }
    health
}

/// Most browser events read for a page's health (a busy page sends thousands of network events).
const MAX_HEALTH_EVENTS: usize = 20_000;

/// What went wrong while a page loaded, from the browser's events: `errors` (uncaught script
/// errors, console errors and error log entries, each text once) and `failed_requests` (answers
/// of 400 or more, and loads that failed). Texts are cut to one line of 300 characters and
/// addresses lose their query strings, so ids in them aren't kept. A page that opened dialogs by
/// itself also gets `dialogs`: each one's kind, first line and how it was answered (`accepted`,
/// `dismissed`, or `waiting` for a question held for the agent when `questions_held`).
pub fn page_health(events: &[CdpEvent], questions_held: bool) -> Value {
    let line = |s: &str| s.lines().next().unwrap_or_default().chars().take(300).collect::<String>();
    let str_at = |v: &Value, p: &str| v.pointer(p).and_then(Value::as_str).unwrap_or_default().to_string();
    let mut urls: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut errors: Vec<Value> = Vec::new();
    let mut failed: Vec<Value> = Vec::new();
    let mut add_error = |kind: &str, text: String, url: String| {
        if !text.is_empty() && errors.len() < 30 && !errors.iter().any(|e| e["text"] == text.as_str()) {
            errors.push(json!({"kind": kind, "text": text, "url": shown_url(&url)}));
        }
    };
    for e in events {
        let p = &e.params;
        match e.method.as_str() {
            "Network.requestWillBeSent" => {
                urls.insert(str_at(p, "/requestId"), str_at(p, "/request/url"));
            }
            "Runtime.exceptionThrown" => {
                let d = &p["exceptionDetails"];
                let text = Some(str_at(d, "/exception/description")).filter(|t| !t.is_empty()).unwrap_or_else(|| str_at(d, "/text"));
                add_error("script", line(&text), str_at(d, "/url"));
            }
            "Runtime.consoleAPICalled" if matches!(p["type"].as_str(), Some("error" | "assert")) => {
                let parts: Vec<String> = p["args"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|a| a.get("value").and_then(Value::as_str).or_else(|| a.get("description").and_then(Value::as_str)).map(String::from))
                    .collect();
                add_error("console", line(&parts.join(" ")), String::new());
            }
            // A failed load is also logged by the browser; the network events say it better.
            "Log.entryAdded" if p.pointer("/entry/level") == Some(&json!("error")) && p.pointer("/entry/source") != Some(&json!("network")) => {
                add_error("log", line(&str_at(p, "/entry/text")), str_at(p, "/entry/url"));
            }
            "Network.responseReceived" => {
                let status = p.pointer("/response/status").and_then(Value::as_u64).unwrap_or(0);
                if status >= 400 && failed.len() < 50 {
                    failed.push(json!({"url": shown_url(&str_at(p, "/response/url")), "status": status, "type": p["type"]}));
                }
            }
            "Network.loadingFailed" if p["canceled"] != true && failed.len() < 50 => {
                let url = urls.get(&str_at(p, "/requestId")).cloned().unwrap_or_default();
                failed.push(json!({"url": shown_url(&url), "error": p["errorText"], "type": p["type"]}));
            }
            _ => {}
        }
    }
    let mut dialogs = web_common::dialogs_from(events);
    // A confirm or prompt held for the agent isn't answered yet.
    if questions_held {
        for d in dialogs.iter_mut().filter(|d| d["answered"] == "dismissed") {
            d["answered"] = json!("waiting");
        }
    }
    let mut health = json!({"errors": errors, "failed_requests": failed});
    if !dialogs.is_empty()
        && let Some(o) = health.as_object_mut()
    {
        o.insert("dialogs".into(), json!(dialogs));
    }
    health
}

/// Evaluate one of our own read-only scripts in the page and return its JSON value.
pub(crate) fn eval(client: &mut CdpClient, script: &str, dl: &Deadline) -> Result<Value, ToolError> {
    let r = client.call("Runtime.evaluate", json!({"expression": script, "returnByValue": true, "awaitPromise": true}), None, dl)?;
    if let Some(ex) = r.get("exceptionDetails") {
        let m = ex.pointer("/exception/description").or_else(|| ex.get("text")).and_then(Value::as_str).unwrap_or("script error");
        // The text is the page's own (its script threw), so it's marked: a caller that must not
        // keep page text can tell this failure apart from a timeout or a lost connection.
        return Err(ToolError::new(ErrorCode::ProgramFailed, format!("reading the page failed: {}", m.lines().next().unwrap_or(m)))
            .with_details(json!({PAGE_SCRIPT_FAILED: true})));
    }
    Ok(r.pointer("/result/value").cloned().unwrap_or(Value::Null))
}

/// The detail [`eval`] sets on an error whose message is the page's own exception text.
const PAGE_SCRIPT_FAILED: &str = "page_script_failed";

/// Whether `e` is [`eval`]'s failure of the page's script, carrying the page's own text.
fn page_script_failed(e: &ToolError) -> bool {
    e.details.as_ref().and_then(|d| d.get(PAGE_SCRIPT_FAILED)).and_then(Value::as_bool) == Some(true)
}

/// The browser events a page's health reads ([`page_health`]), kept up to [`MAX_HEALTH_EVENTS`].
fn keep_health_events(seen: &mut Vec<CdpEvent>, events: Vec<CdpEvent>) {
    let wanted = events
        .into_iter()
        .filter(|e| e.method.starts_with("Runtime.") || e.method.starts_with("Log.") || e.method.starts_with("Network.") || e.method == cdp::DIALOG_OPENING);
    seen.extend(wanted.take(MAX_HEALTH_EVENTS.saturating_sub(seen.len())));
}

fn page_facts(client: &mut CdpClient, dl: &Deadline) -> Result<Value, ToolError> {
    eval(
        client,
        "({url: location.href, title: document.title, ready: document.readyState, lang: document.documentElement.lang || '', \
          width: document.documentElement.scrollWidth, height: document.documentElement.scrollHeight, \
          elements: document.getElementsByTagName('*').length, images: document.images.length, links: document.links.length, \
          forms: document.forms.length})",
        dl,
    )
}

// ---------------------------------------------------------------------------------------------
// web_page_dialog_answer

/// The id of the tool that answers a page's question; named in summaries and remediations.
pub const DIALOG_ANSWER_TOOL: &str = "web_page_dialog_answer";

/// Longest prompt answer accepted.
const MAX_PROMPT_TEXT: usize = 10_000;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DialogAnswerParams {
    /// true agrees (OK), false dismisses (Cancel).
    pub accept: bool,
    /// For a prompt: the text to answer with (accepted only). Not kept in the evidence itself;
    /// what the page shows or logs afterwards (its title, a console error) can repeat it.
    #[serde(default)]
    pub prompt_text: Option<String>,
    /// Extra time to let the page settle after the answer, in milliseconds (default 2000).
    #[serde(default = "d_settle")]
    pub settle_ms: u64,
    #[serde(default = "d_timeout")]
    pub timeout_secs: u64,
}

/// Answer the question the open website is waiting on, then report where the page ended up.
fn web_page_dialog_answer(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: DialogAnswerParams = parse_params(params)?;
    if p.prompt_text.as_ref().is_some_and(|t| t.len() > MAX_PROMPT_TEXT) {
        return Err(ToolError::invalid(format!("prompt_text is longer than {MAX_PROMPT_TEXT} bytes")));
    }
    if p.prompt_text.is_some() && !p.accept {
        return Err(ToolError::invalid("prompt_text is typed only when the prompt is accepted")
            .with_remediation("Pass accept: true with prompt_text, or leave prompt_text out."));
    }
    let dl = deadline(ctx, p.timeout_secs)?;
    // A held question is answered on its own connection, which says itself if the browser is gone.
    // The liveness probe isn't asked first: one that fails would drop the site, and with it the
    // only connection that can read an earlier answer the browser hadn't confirmed.
    let site = match ctx.session.ext.get::<SiteBrowser>().filter(|s| held_dialog(s).is_some()) {
        Some(site) => site,
        None => open_site(ctx, &dl)?,
    };
    // Only the connection that saw the question can answer it (HeldDialog).
    let Some(HeldDialog { open: asked, mut client }) = take_held(&site) else {
        return Err(ToolError::new(ErrorCode::NotFound, "The page isn't waiting on a question, so there's nothing to answer.")
            .with_remediation("A question the website asks is reported as dialog_open by web_site_open or web_site_capture; answer it then."));
    };
    let mut answer = json!({"accept": p.accept});
    if let (Some(t), Some(o)) = (&p.prompt_text, answer.as_object_mut()) {
        o.insert("promptText".into(), json!(t));
    }
    // What the client saw before the answer stays queued, except the held question's own opening:
    // it was reported when it was held, and read again after the answer it would be listed twice.
    // The rest is read after the answer (a load already under way included) and, if the answer
    // isn't confirmed, stays with the held connection for the next try. A question that opens
    // after the answer is a new one and is read as such.
    client.forget_queued_questions();
    let mut seen: Vec<CdpEvent> = Vec::new();
    // What the page took: this answer, or one sent before whose reply came late (then this one
    // isn't sent, so it can't land on a question the page asked since).
    let (taken, earlier) = match client.answer_dialog(answer, &dl) {
        Ok(DialogAnswer::Answered) => (p.accept, false),
        Ok(DialogAnswer::AnsweredBefore { accept }) => (accept.unwrap_or(p.accept), true),
        // The browser says so when nothing is showing any more (the page closed it itself).
        Ok(DialogAnswer::NotShowing(e)) => {
            cleanup(&mut client, &dl);
            return Err(ToolError::new(ErrorCode::NotFound, "The page is no longer waiting on its question, so there's nothing to answer.")
                .with_remediation("Open the website again with web_site_open to see where it is.")
                .with_details(json!({"browser": e.message})));
        }
        // No word either way (out of time, stopped): the question may still be showing, and this
        // connection is the only one that can answer it, so it's kept for another try.
        // The question may have closed before the reply (its closed event came first): the
        // answer's outcome is still unread, and only this connection can read it.
        Err(e) if client.connected() && (client.waiting_dialog().is_some() || client.answer_unconfirmed()) => {
            hold(&site, asked, client);
            return Err(ToolError::new(e.code, format!("The browser didn't confirm the answer to the page's question: {}", e.message)).with_remediation(
                format!("The page is still waiting on it: call {DIALOG_ANSWER_TOOL} again, with a longer timeout_secs if the browser is slow."),
            ));
        }
        Err(e) => {
            cleanup(&mut client, &dl);
            return Err(ToolError::new(e.code, format!("The connection to the page was lost while answering its question: {}", e.message))
                .with_remediation("Open the website again with web_site_open to see where it is."));
        }
    };
    let asked = Some(asked);
    let mut unread: Option<String> = None;
    let r = (|| -> Result<(Value, Value, bool), ToolError> {
        client.call("Page.enable", json!({}), None, &dl)?;
        for domain in ["Runtime.enable", "Log.enable", "Network.enable"] {
            let _ = client.call(domain, json!({}), None, &dl);
        }
        let loading = wait_after_answer(&mut client, p.settle_ms, &mut seen, &dl)?;
        let health = health_with_dialog(&client, &seen);
        if health.get("dialog_open").is_some() {
            return Ok((health, Value::Null, false));
        }
        // A read started with no time left fails before it's sent: say so instead.
        if dl.expired() {
            return Err(ToolError::new(ErrorCode::TimedOut, "no time was left to read it"));
        }
        let facts = page_facts(&mut client, &dl)?;
        // What the page reported while it was read (a console error) belongs to its health too.
        keep_health_events(&mut seen, client.take_events());
        Ok((health_with_dialog(&client, &seen), facts, loading))
    })();
    // A question asked while we set up or read the page is held by the client: the page stopped
    // there. Its opening may still be queued; read it now with what was seen before, or the next
    // answer would take it for something said after.
    let r = match r {
        Err(_) if client.waiting_dialog().is_some() => {
            keep_health_events(&mut seen, client.take_events());
            Ok((health_with_dialog(&client, &seen), Value::Null, false))
        }
        // The answer is given either way: a failure to read the page after it (out of time, a
        // lost connection) is said with the answer, not returned as the tool failing, which would
        // leave a retry nothing to answer. The page's own exception text isn't kept: it could
        // repeat what was typed into the prompt, which the evidence leaves out.
        Err(e) => {
            unread = Some(if page_script_failed(&e) { "the page's script failed while it was read".to_string() } else { e.message });
            keep_health_events(&mut seen, client.take_events());
            Ok((health_with_dialog(&client, &seen), Value::Null, false))
        }
        r => r,
    };
    // The wait ended at its bound with a frame still loading: what was read may be mid-load.
    let still_loading = matches!(&r, Ok((_, _, true)));
    let r = r.map(|(h, f, _)| (h, f));
    let next = client.waiting_dialog().map(dialog_open);
    match &next {
        Some(open) => hold(&site, open.clone(), client),
        None => cleanup(&mut client, &dl),
    }
    // A later capture records the page's health: it gets the answer and what the page did after.
    let after = r.as_ref().map(|(h, _)| h.clone()).unwrap_or(Value::Null);
    if let Ok(mut h) = site.health.lock() {
        let merged = health_after_answer(&h, taken, &after);
        *h = merged;
    }
    let (health, facts) = r?;
    let here = facts.get("url").and_then(Value::as_str).map(str::to_string);
    if let (Some(u), Ok(mut slot)) = (&here, site.url.lock()) {
        slot.clone_from(u);
    }
    let shown = shown_url(&here.clone().unwrap_or_else(|| site.url.lock().map(|u| u.clone()).unwrap_or_default()));
    let title = facts.get("title").and_then(Value::as_str).unwrap_or_default().to_string();
    let what = asked.as_ref().map_or_else(
        || "the page's question".to_string(),
        |a| format!("the page's {} (\"{}\")", if a["type"] == "prompt" { "prompt" } else { "confirm question" }, a["message"].as_str().unwrap_or_default()),
    );
    let verb = if taken { "Accepted" } else { "Dismissed" };
    let typed = match (earlier, p.prompt_text.is_some()) {
        (true, _) => " with the answer sent before, which the browser took late (this one wasn't sent)",
        (false, true) => " with the text given",
        (false, false) => "",
    };
    let now = if next.is_some() {
        String::new()
    } else if title.is_empty() {
        format!(" The page is now at {shown}.")
    } else {
        format!(" The page is now at {shown}: \"{title}\".")
    };
    let dialogs = health.get("dialogs").cloned().unwrap_or_else(|| json!([]));
    let now = match &unread {
        Some(why) => format!(" The page wasn't read afterwards ({why})."),
        None => now,
    };
    let summary = format!("{verb} {what}{typed}.{now}{}{}", dialogs_said(&dialogs), next.as_ref().map(dialog_waiting_said).unwrap_or_default());
    let visitor = ctx.session.clean_room();
    let browser = site.handle();
    let mut ev =
        EvidenceBuilder::new(DIALOG_ANSWER_TOOL, Subject { kind: "website".into(), sha256: None, name: Some(shown.clone()) }, browser_env(visitor, &browser))
            // What was typed into a prompt is the agent's answer, not the site's: only that one was given.
            .params(json!({"accept": p.accept, "prompt_text_given": p.prompt_text.is_some()}))
            .result(json!({
                "answered": {"dialog": asked, "accept": taken, "earlier": earlier},
                "url": here.map(|u| shown_url(&u)),
                "title": title,
                "dialogs": dialogs,
                "dialog_open": next,
                "health": if visitor { Value::Null } else { health.clone() },
            }))
            .summary(summary)
            .confidence(Confidence::Observed)
            .authority(Authority::OurRun)
            .location(Location::new("url", shown));
    ev = match &next {
        Some(_) => {
            ev.limitation(waiting_limitation()).coverage(Coverage { status: CoverageStatus::Partial, examined: Some(0), total: Some(1), next_offset: None })
        }
        None => match &unread {
            Some(why) => ev
                .limitation(Limitation::new(
                    "page_unread",
                    format!("The answer was given, but the page wasn't read afterwards ({why}); open the website again with web_site_open to see where it is."),
                ))
                .coverage(Coverage { status: CoverageStatus::Partial, examined: Some(0), total: Some(1), next_offset: None }),
            None if still_loading => ev
                .limitation(Limitation::new(
                    "page_still_loading",
                    "The page was still loading when the wait after the answer ended (45 s at most), so what was read may change as it finishes; open the website again with web_site_open to see it settled.",
                ))
                .coverage(Coverage { status: CoverageStatus::Partial, examined: Some(1), total: Some(1), next_offset: None }),
            None => ev.coverage(Coverage::complete(1)),
        },
    };
    Ok(ToolOutput::evidence(ev.build()?))
}

/// After an answer the page may go on loading (a navigation it asked about) or stay where it is:
/// wait for a load that started, at most `AFTER_ANSWER_LOAD_WAIT`, and at least `settle_ms`.
/// True when the wait ended with a frame still loading (its bound reached, or the deadline): the
/// page may be read mid-load, which the result says.
fn wait_after_answer(client: &mut CdpClient, settle_ms: u64, seen: &mut Vec<CdpEvent>, dl: &Deadline) -> Result<bool, ToolError> {
    wait_after_answer_within(client, settle_ms, AFTER_ANSWER_LOAD_WAIT, seen, dl)
}

/// The longest wait for a page's load after an answer.
const AFTER_ANSWER_LOAD_WAIT: Duration = Duration::from_secs(45);

fn wait_after_answer_within(client: &mut CdpClient, settle_ms: u64, bound: Duration, seen: &mut Vec<CdpEvent>, dl: &Deadline) -> Result<bool, ToolError> {
    let quiet = Deadline::new(Duration::from_millis(settle_ms.min(30_000)), crate::tool::CancelToken::default());
    let longest = Deadline::new(bound, crate::tool::CancelToken::default());
    // Frame loads are tracked by the client as its events arrive (CdpClient::frames_loading), so
    // a load that started before the answer, or before an earlier question, is still waited for.
    // The events that came while the answer and setup awaited their replies are read first, so
    // no settle time (settle_ms: 0) skips only the extra wait, not what was already seen.
    let mut queued = client.take_events();
    loop {
        keep_health_events(seen, queued);
        if client.waiting_dialog().is_some() {
            return Ok(false);
        }
        if !client.frames_loading() && quiet.expired() {
            return Ok(false);
        }
        if longest.expired() || dl.expired() {
            return Ok(client.frames_loading());
        }
        client.pump_until(&Deadline::new(Duration::from_millis(250), crate::tool::CancelToken::default()))?;
        queued = client.take_events();
    }
}

// ---------------------------------------------------------------------------------------------
// web_page_states_capture

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StatesParams {
    /// Absolute path of a folder that does not exist yet; the states are written there.
    pub output_directory: String,
    /// Most controls to open (1..=8, default 4).
    #[serde(default = "d_states")]
    pub max_states: usize,
    /// Page width in CSS pixels (320..=3840, default 1440).
    #[serde(default = "d_state_width")]
    pub width: u32,
    #[serde(default = "d_timeout")]
    pub timeout_secs: u64,
}

fn d_states() -> usize {
    4
}
fn d_state_width() -> u32 {
    1440
}

/// Marks the visible toggles (`data-craft-state="<n>"`) and lists them, after dropping the marks
/// an earlier call left (they'd name a control hidden at this width, #339). `__MAX__` is replaced.
const FIND_STATES_JS: &str = r#"(() => {
  for (const old of document.querySelectorAll('[data-craft-state]')) old.removeAttribute('data-craft-state');
  const cap = (s, n) => String(s || '').replace(/\s+/g, ' ').trim().slice(0, n);
  const shown = el => { const r = el.getBoundingClientRect(); if (r.width < 1 || r.height < 1) return false; const s = getComputedStyle(el); return s.visibility !== 'hidden' && s.display !== 'none'; };
  const sel = '[aria-expanded="false"], [aria-haspopup]:not([aria-haspopup="false"]), [role="tab"][aria-selected="false"], details:not([open]) > summary';
  const out = [];
  for (const el of document.querySelectorAll(sel)) {
    if (out.length >= __MAX__) break;
    if (!shown(el) || el.closest('form') || el.matches('[type="submit"]')) continue;
    const pop = el.getAttribute('aria-haspopup');
    const kind = el.matches('summary') ? 'section' : el.getAttribute('role') === 'tab' ? 'tab' : pop === 'dialog' ? 'dialog' : 'menu';
    el.setAttribute('data-craft-state', String(out.length));
    out.push({index: out.length, kind, label: cap(el.innerText || el.getAttribute('aria-label') || el.getAttribute('title'), 80)});
  }
  return out;
})()"#;

/// What the page looks like around control `__I__` now.
const STATE_JS: &str = r#"(() => {
  const el = document.querySelector('[data-craft-state="__I__"]');
  const seen = d => { const r = d.getBoundingClientRect(); return r.width > 0 && r.height > 0; };
  const dialogs = Array.from(document.querySelectorAll('[role="dialog"], [role="alertdialog"], dialog[open], [aria-modal="true"]')).filter(seen).length;
  const details = el && el.parentElement && el.parentElement.tagName === 'DETAILS' ? el.parentElement.open : null;
  return {present: !!el, expanded: el ? el.getAttribute('aria-expanded') : null, selected: el ? el.getAttribute('aria-selected') : null, open: details, dialogs, url: location.href};
})()"#;

const CLICK_STATE_JS: &str = r#"(() => { const el = document.querySelector('[data-craft-state="__I__"]'); if (!el) return false; el.scrollIntoView({block: 'center'}); el.click(); return true; })()"#;

/// Before tab `__I__` is clicked: mark the tab its list has selected now, to select it again after (#335).
const REMEMBER_TAB_JS: &str = r#"(() => {
  const el = document.querySelector('[data-craft-state="__I__"]');
  if (!el || el.getAttribute('role') !== 'tab') return false;
  const list = el.closest('[role="tablist"]') || el.parentElement;
  const was = list && Array.from(list.querySelectorAll('[role="tab"][aria-selected="true"]')).find(t => t !== el);
  if (!was) return false;
  was.setAttribute('data-craft-tab-was', '__I__');
  return true;
})()"#;

const RESELECT_TAB_JS: &str = r#"(() => { const el = document.querySelector('[data-craft-tab-was="__I__"]'); if (!el) return false; el.removeAttribute('data-craft-tab-was'); el.click(); return true; })()"#;

fn press_escape(client: &mut CdpClient, dl: &Deadline) {
    for kind in ["keyDown", "keyUp"] {
        let _ = client.call("Input.dispatchKeyEvent", json!({"type": kind, "key": "Escape", "code": "Escape", "windowsVirtualKeyCode": 27}), None, dl);
    }
}

fn settle(client: &mut CdpClient, ms: u64) -> Result<(), ToolError> {
    let d = Deadline::new(Duration::from_millis(ms), crate::tool::CancelToken::default());
    client.pump_until(&d)?;
    let _ = client.take_events();
    Ok(())
}

/// What a click changed, in plain words, from the state before and after.
fn state_change(before: &Value, after: &Value) -> Vec<&'static str> {
    let mut out = Vec::new();
    if after["expanded"] == "true" && before["expanded"] != "true" {
        out.push("expanded");
    }
    if after["selected"] == "true" && before["selected"] != "true" {
        out.push("tab selected");
    }
    if after["open"] == true && before["open"] != true {
        out.push("section opened");
    }
    if after["dialogs"].as_u64().unwrap_or(0) > before["dialogs"].as_u64().unwrap_or(0) {
        out.push("dialog appeared");
    }
    if after["url"] != before["url"] {
        out.push("address changed");
    }
    out
}

fn web_page_states_capture(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: StatesParams = parse_params(params)?;
    if !(1..=8).contains(&p.max_states) {
        return Err(ToolError::invalid("max_states must be 1..=8"));
    }
    if !(320..=3840).contains(&p.width) {
        return Err(ToolError::invalid("width must be 320..=3840"));
    }
    let dl = deadline(ctx, p.timeout_secs.max(30))?;
    let site = open_site(ctx, &dl)?;
    refuse_if_waiting(&site)?;
    let out = create_absent_dir(&p.output_directory)?;
    // A question one of our clicks opens is dismissed: this tool never agrees to what a control asks.
    let mut client = page_client_dismissing(&site, &dl)?;
    let r = (|| -> Result<(String, Vec<Value>, usize), ToolError> {
        client.call("Page.enable", json!({}), None, &dl)?;
        set_viewport(&mut client, p.width, 900, &dl)?;
        settle(&mut client, 500)?;
        let start = eval(&mut client, "location.href", &dl)?.as_str().unwrap_or_default().to_string();
        let found = eval(&mut client, &FIND_STATES_JS.replace("__MAX__", &p.max_states.to_string()), &dl)?;
        let found = found.as_array().cloned().unwrap_or_default();
        let mut states = Vec::new();
        for (n, c) in found.iter().enumerate() {
            if dl.expired() {
                break;
            }
            let i = c.get("index").and_then(Value::as_u64).unwrap_or(n as u64).to_string();
            ctx.report(Some(n as f32 / found.len().max(1) as f32), format!("Opening {}…", c.get("label").and_then(Value::as_str).unwrap_or("a control")));
            let before = eval(&mut client, &STATE_JS.replace("__I__", &i), &dl)?;
            let _ = eval(&mut client, &REMEMBER_TAB_JS.replace("__I__", &i), &dl);
            if eval(&mut client, &CLICK_STATE_JS.replace("__I__", &i), &dl)? != true {
                continue;
            }
            settle(&mut client, 700)?;
            let after = eval(&mut client, &STATE_JS.replace("__I__", &i), &dl).unwrap_or(Value::Null);
            let changed = state_change(&before, &after);
            let mut row = json!({"index": n, "kind": c.get("kind"), "label": c.get("label"), "changed": changed});
            if !changed.contains(&"address changed") {
                let shot = client.call("Page.captureScreenshot", json!({"format": "png"}), None, &dl)?;
                if let Some(data) = shot.get("data").and_then(Value::as_str) {
                    let bytes = web_common::b64_decode(data)?;
                    let name = format!("state-{}.png", n + 1);
                    write_new_file(&out, std::slice::from_ref(&name), &bytes).map_err(ToolError::invalid)?;
                    row["file"] = json!(name);
                    row["sha256"] = json!(sha256_hex(&bytes));
                }
            }
            // Put the page back for the next one: close what opened, or go back to where it was.
            press_escape(&mut client, &dl);
            if after["url"].as_str().is_some_and(|u| u != start) {
                let _ = navigate(&mut client, &start, p.width, 900, 800, &dl)?;
                // The page reloaded: its marks are gone, so mark the toggles again.
                let _ = eval(&mut client, &FIND_STATES_JS.replace("__MAX__", &p.max_states.to_string()), &dl);
            } else if after["expanded"] == "true" || after["open"] == true {
                let _ = eval(&mut client, &CLICK_STATE_JS.replace("__I__", &i), &dl);
            } else if changed.contains(&"tab selected") {
                let _ = eval(&mut client, &RESELECT_TAB_JS.replace("__I__", &i), &dl);
            }
            settle(&mut client, 300)?;
            states.push(row);
        }
        Ok((start, states, found.len()))
    })();
    cleanup(&mut client, &dl);
    let (start, states, found) = r?;
    let json_bytes = serde_json::to_vec_pretty(&json!({"url": start, "width": p.width, "states": states})).map_err(|e| ToolError::internal(e.to_string()))?;
    write_new_file(&out, &["states.json".into()], &json_bytes).map_err(ToolError::invalid)?;
    let shown = shown_url(&start);
    let pictured = states.iter().filter(|s| s.get("file").is_some()).count();
    let summary = if found == 0 {
        format!("{shown} has no menus, pop-ups, tabs or collapsed sections to open.")
    } else {
        format!("Opened {} of {found} control(s) on {shown}; {pictured} pictured in {}.", states.len(), out.display())
    };
    let browser = site.handle();
    let ev = EvidenceBuilder::new(
        "web_page_states_capture",
        Subject { kind: "website".into(), sha256: None, name: Some(shown.clone()) },
        browser_env(ctx.session.clean_room(), &browser),
    )
    .params(json!({"max_states": p.max_states, "width": p.width}))
    .result(json!({"url": start, "found": found, "states": states, "folder": out.display().to_string()}))
    .summary(summary)
    .confidence(Confidence::Observed)
    .authority(Authority::OurRun)
    .limitation(Limitation::new("first_controls", "Only the first controls found are opened, one at a time; menus inside menus and hover-only menus are not."))
    .coverage(Coverage::complete(states.len() as u64))
    .location(Location::new("url", start))
    .build()?;
    Ok(ToolOutput::evidence(ev))
}

// ---------------------------------------------------------------------------------------------
// web_site_capture

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CaptureParams {
    /// Absolute path of a folder that does not exist yet; the capture is written there.
    pub output_directory: String,
    /// Page widths to screenshot, in CSS pixels (1..=4 widths, each 320..=3840). Default: desktop and phone.
    #[serde(default = "d_widths")]
    pub widths: Vec<u32>,
    /// Tallest screenshot in CSS pixels; longer pages are cut here (600..=20000, default 12000).
    #[serde(default = "d_max_height")]
    pub max_height: u32,
    /// Keep the rendered HTML (default true).
    #[serde(default = "yes")]
    pub include_html: bool,
    /// Photograph each width a second time a moment later and save the parts that changed by
    /// themselves (carousels, videos, animations) as noise-<width>.png, white where it changed,
    /// so comparisons can leave them out (default false).
    #[serde(default)]
    pub noise_pass: bool,
    #[serde(default = "d_timeout")]
    pub timeout_secs: u64,
}

fn d_widths() -> Vec<u32> {
    vec![1440, 390]
}
fn d_max_height() -> u32 {
    12_000
}

const MAX_HTML_BYTES: usize = 4 << 20;

/// Our read-only blueprint script. Counts styles, outlines visible structure and lists assets;
/// form fields contribute only their type, name, label and placeholder.
const BLUEPRINT_JS: &str = r#"(() => {
  const cap = (s, n) => String(s || '').replace(/\s+/g, ' ').trim().slice(0, n);
  const box = el => { const r = el.getBoundingClientRect(); return {x: Math.round(r.x + scrollX), y: Math.round(r.y + scrollY), w: Math.round(r.width), h: Math.round(r.height)}; };
  const shown = el => { const r = el.getBoundingClientRect(); if (r.width < 1 || r.height < 1) return false; const s = getComputedStyle(el); return s.visibility !== 'hidden' && s.display !== 'none' && s.opacity !== '0'; };
  const tally = (m, k) => { if (k) m[k] = (m[k] || 0) + 1; };
  const top = (m, n) => Object.entries(m).sort((a, b) => b[1] - a[1]).slice(0, n).map(([value, count]) => ({value, count}));
  const fonts = {}, sizes = {}, weights = {}, colors = {}, backgrounds = {}, radii = {}, shadows = {}, bgImages = {}, spacing = {}, maxWidths = {};
  // Colours as #rrggbb(aa), whatever notation the CSS used. Computed rgb()/rgba() is read as numbers: a canvas read
  // can be off by one per channel (Brave adds noise to it, #154). Anything else (oklch, color()…) is painted on a 1×1
  // canvas and read back.
  const cv = document.createElement('canvas'); cv.width = cv.height = 1; const g = cv.getContext('2d', {willReadFrequently: true}); const seenHex = {};
  const hex = c => { if (!c) return c; if (c in seenHex) return seenHex[c]; const h2 = x => x.toString(16).padStart(2, '0'); const m = /^rgba?\((\d+), (\d+), (\d+)(?:, ([\d.]+))?\)$/.exec(c); if (m) { const a = m[4] === undefined ? 1 : +m[4]; return seenHex[c] = '#' + h2(+m[1]) + h2(+m[2]) + h2(+m[3]) + (a < 1 ? h2(Math.round(a * 255)) : ''); } if (!g) return c; g.clearRect(0, 0, 1, 1); g.fillStyle = '#000'; g.fillStyle = c; g.fillRect(0, 0, 1, 1); const d = g.getImageData(0, 0, 1, 1).data; return seenHex[c] = '#' + h2(d[0]) + h2(d[1]) + h2(d[2]) + (d[3] < 255 ? h2(d[3]) : ''); };
  const els = [document.documentElement, document.body, ...Array.from(document.querySelectorAll('body *')).slice(0, 12000)].filter(Boolean);
  for (const el of els) {
    if (!shown(el)) continue;
    const s = getComputedStyle(el);
    const text = Array.from(el.childNodes).some(n => n.nodeType === 3 && n.textContent.trim());
    if (text) { tally(fonts, s.fontFamily); tally(sizes, s.fontSize); tally(weights, s.fontWeight); tally(colors, hex(s.color)); }
    if (s.backgroundColor && s.backgroundColor !== 'rgba(0, 0, 0, 0)' && s.backgroundColor !== 'transparent') tally(backgrounds, hex(s.backgroundColor));
    for (const k of ['paddingTop', 'paddingRight', 'paddingBottom', 'paddingLeft', 'rowGap', 'columnGap', 'marginTop', 'marginBottom']) { const v = s[k]; if (v && v.endsWith('px') && v !== '0px') tally(spacing, v); }
    if (s.maxWidth.endsWith('px')) tally(maxWidths, s.maxWidth);
    // A pill made with calc(infinity * 1px) comes back clamped as 3.35544e+07px: record it as 9999px (#155).
    if (s.borderRadius && s.borderRadius !== '0px') tally(radii, s.borderRadius.replace(/[\d.]+(?:e\+?\d+)?px/g, n => parseFloat(n) > 9999 ? '9999px' : n));
    if (s.boxShadow && s.boxShadow !== 'none') tally(shadows, s.boxShadow);
    const bi = s.backgroundImage; if (bi && bi !== 'none') for (const m of bi.matchAll(/url\(["']?([^"')]+)["']?\)/g)) tally(bgImages, m[1]);
  }
  const vars = {}, bps = {}; let ruleCount = 0, unreadable = 0;
  const bp = (side, n, unit) => tally(bps, side + '-width ' + Math.round(parseFloat(n) * (unit === 'px' ? 1 : 16)) + 'px');
  const walk = (rules, depth) => {
    if (!rules || depth > 6) return;
    for (const r of Array.from(rules)) {
      if (++ruleCount > 40000) return;
      if (r.selectorText && r.selectorText.includes(':root') && r.style) for (const name of Array.from(r.style)) if (name.startsWith('--') && Object.keys(vars).length < 300) vars[name] = cap(r.style.getPropertyValue(name), 120);
      const media = r.media && r.media.mediaText ? r.media.mediaText : (r.conditionText || '');
      for (const m of media.matchAll(/(min|max)-width\s*:\s*([\d.]+)(px|r?em)/g)) bp(m[1], m[2], m[3]);
      for (const m of media.matchAll(/width\s*(>=?|<=?)\s*([\d.]+)(px|r?em)/g)) bp(m[1][0] === '>' ? 'min' : 'max', m[2], m[3]);
      if (r.cssRules) walk(r.cssRules, depth + 1);
    }
  };
  for (const sheet of Array.from(document.styleSheets)) { let rules; try { rules = sheet.cssRules; } catch (e) { unreadable++; continue; } walk(rules, 0); }
  const breakpoints = Object.entries(bps).map(([query, count]) => ({query, px: parseInt(query.split(' ')[1], 10), count})).sort((a, b) => a.px - b.px).slice(0, 24);
  const PICK = 'header,nav,main,footer,section,article,aside,form,h1,h2,h3,h4,p,ul,ol,button,a,img,picture,video,svg,input,select,textarea,[role]';
  const outline = [];
  for (const el of document.querySelectorAll(PICK)) {
    if (outline.length >= 900) break;
    if (!shown(el)) continue;
    let depth = 0; for (let p = el.parentElement; p; p = p.parentElement) depth++;
    const tag = el.tagName.toLowerCase();
    const item = {tag, depth, box: box(el)};
    const role = el.getAttribute('role'); if (role) item.role = role;
    if (el.id) item.id = cap(el.id, 60);
    const cls = typeof el.className === 'string' ? cap(el.className, 120) : ''; if (cls) item.class = cls;
    if (!['main', 'section', 'article', 'header', 'footer', 'nav', 'aside', 'form', 'ul', 'ol', 'svg', 'picture', 'video'].includes(tag)) { const t = cap(el.innerText, 200); if (t) item.text = t; }
    if (tag === 'a') item.href = el.href || '';
    if (tag === 'img') { item.src = el.currentSrc || el.src || ''; item.alt = cap(el.alt, 120); }
    if (['input', 'select', 'textarea'].includes(tag)) { item.type = el.type || tag; item.name = cap(el.name, 60); item.placeholder = cap(el.placeholder, 80); }
    const s = getComputedStyle(el);
    if (['header', 'nav', 'main', 'footer', 'section', 'article', 'aside', 'form', 'ul', 'ol'].includes(tag)) { item.display = s.display; if (s.display.includes('flex')) item.flex = s.flexDirection; if (s.display.includes('grid')) item.grid = cap(s.gridTemplateColumns, 120); }
    item.font = s.fontSize + ' ' + s.fontWeight; item.color = hex(s.color); if (s.backgroundColor !== 'rgba(0, 0, 0, 0)') item.background = hex(s.backgroundColor);
    outline.push(item);
  }
  const labelFor = el => { if (el.labels && el.labels[0]) return cap(el.labels[0].innerText, 80); return cap(el.getAttribute('aria-label'), 80); };
  const forms = Array.from(document.forms).slice(0, 30).map(f => ({action: f.getAttribute('action') || '', method: (f.method || 'get').toLowerCase(),
    fields: Array.from(f.elements).slice(0, 60).map(e => ({tag: e.tagName.toLowerCase(), type: e.type || '', name: cap(e.name, 60), label: labelFor(e), placeholder: cap(e.placeholder, 80), required: !!e.required}))}));
  const links = Array.from(document.links).slice(0, 400).map(a => ({text: cap(a.innerText || a.getAttribute('aria-label'), 100), href: a.href}));
  const meta = n => { const m = document.querySelector(`meta[name="${n}"],meta[property="${n}"]`); return m ? cap(m.content, 300) : ''; };
  const icons = Array.from(document.querySelectorAll('link[rel~="icon"],link[rel="apple-touch-icon"]')).map(l => l.href).slice(0, 10);
  // What the page loaded, without query strings (they can carry ids).
  const resources = (performance.getEntriesByType ? performance.getEntriesByType('resource') : []).map(e => ({url: String(e.name).split(/[?#]/)[0], type: e.initiatorType, bytes: e.transferSize || 0}));
  // Accessibility at a glance: controls a screen reader can't name, pictures without alt text,
  // the landmarks and the page language.
  const CONTROLS = 'a[href],button,input:not([type="hidden"]),select,textarea,[role="button"],[role="link"],[role="tab"],[role="menuitem"]';
  const named = el => !!(cap(el.innerText, 80) || el.getAttribute('aria-label') || el.getAttribute('aria-labelledby') || el.getAttribute('title')
    || (el.labels && el.labels.length) || el.getAttribute('placeholder') || (['submit', 'button', 'reset'].includes(el.type) && el.value)
    || el.querySelector('img[alt]:not([alt=""]),svg title,[aria-label]'));
  const unnamed = Array.from(document.querySelectorAll(CONTROLS)).slice(0, 3000).filter(el => shown(el) && !named(el));
  const landmarks = {};
  for (const el of document.querySelectorAll('header,nav,main,footer,aside,form[aria-label],[role="banner"],[role="navigation"],[role="main"],[role="contentinfo"],[role="complementary"],[role="search"]')) {
    if (!shown(el)) continue;
    const role = el.getAttribute('role') || {header: 'banner', nav: 'navigation', main: 'main', footer: 'contentinfo', aside: 'complementary', form: 'form'}[el.tagName.toLowerCase()];
    tally(landmarks, role);
  }
  const a11y = {lang: document.documentElement.lang || '', landmarks, unnamed_controls: unnamed.length,
    unnamed_examples: unnamed.slice(0, 5).map(el => el.tagName.toLowerCase() + (el.className && typeof el.className === 'string' ? '.' + cap(el.className, 40).split(' ')[0] : '')),
    images_without_alt: Array.from(document.images).filter(i => shown(i) && !i.hasAttribute('alt')).length};
  const fontFaces = []; try { document.fonts.forEach(f => { if (fontFaces.length < 80) fontFaces.push({family: f.family, weight: f.weight, style: f.style, status: f.status}); }); } catch (e) {}
  return {
    url: location.href, title: document.title, lang: document.documentElement.lang || '',
    viewport: {w: innerWidth, h: innerHeight}, page: {w: document.documentElement.scrollWidth, h: document.documentElement.scrollHeight},
    meta: {description: meta('description'), theme_color: meta('theme-color'), og_title: meta('og:title'), og_image: meta('og:image'), icons},
    design: {fonts: top(fonts, 12), font_sizes: top(sizes, 16), font_weights: top(weights, 8), text_colors: top(colors, 16), backgrounds: top(backgrounds, 16), radii: top(radii, 10), shadows: top(shadows, 8), spacing: top(spacing, 24), max_widths: top(maxWidths, 8), css_variables: vars, font_faces: fontFaces},
    breakpoints, unreadable_stylesheets: unreadable, a11y,
    outline, forms, links,
    assets: {
      images: Array.from(document.images).slice(0, 300).map(i => ({src: i.currentSrc || i.src, w: i.naturalWidth, h: i.naturalHeight, alt: cap(i.alt, 120)})),
      background_images: top(bgImages, 60).map(x => x.value),
      stylesheets: Array.from(document.styleSheets).map(s => s.href).filter(Boolean).slice(0, 80),
      scripts: Array.from(document.scripts).map(s => s.src).filter(Boolean).slice(0, 120),
      inline_scripts: Array.from(document.scripts).filter(s => !s.src).length,
      fonts: resources.filter(r => /\.(woff2?|ttf|otf|eot)$/i.test(r.url) || r.url.includes('fonts.googleapis.com')).map(r => r.url).slice(0, 40),
      requests: resources.slice(0, 300)
    },
    headings: Array.from(document.querySelectorAll('h1,h2,h3')).slice(0, 120).map(h => ({level: Number(h.tagName[1]), text: cap(h.innerText, 160)})),
    text: cap(document.body ? document.body.innerText : '', 30000)
  };
})()"#;

/// Scroll through the page as a visitor would, so sections that appear as they come into view and
/// lazy images are there for the full-page picture (#160), then back to the top. At most a few
/// seconds; returns how far it scrolled.
const REVEAL_JS: &str = r#"(async () => {
  const pause = ms => new Promise(r => setTimeout(r, ms));
  const end = Date.now() + 5000;
  const step = Math.max(200, Math.floor(innerHeight * 0.8));
  const bottom = () => Math.max(document.documentElement.scrollHeight, document.body ? document.body.scrollHeight : 0) - innerHeight;
  let y = 0;
  while (y < bottom() && Date.now() < end) {
    y = Math.min(y + step, bottom());
    scrollTo(0, y);
    await pause(120);
  }
  for (const img of document.images) if (img.loading === 'lazy') img.loading = 'eager';
  const loading = [...document.images].filter(i => !i.complete).map(i => new Promise(r => { i.addEventListener('load', r, {once: true}); i.addEventListener('error', r, {once: true}); }));
  await Promise.race([Promise.all(loading), pause(Math.max(300, end - Date.now()))]);
  scrollTo(0, 0);
  await pause(200);
  return y;
})()"#;

/// Our read-only layout script, run at every width: the page's width (wider than the window means
/// it scrolls sideways), heading and text sizes, and each landmark and section's padding, gap,
/// background, content column and how many items sit side by side.
const LAYOUT_JS: &str = r#"(() => {
  const cap = (s, n) => String(s || '').replace(/\s+/g, ' ').trim().slice(0, n);
  const px = v => Math.round(parseFloat(v) || 0);
  const shown = el => { const r = el.getBoundingClientRect(); if (r.width < 1 || r.height < 1) return false; const s = getComputedStyle(el); return s.visibility !== 'hidden' && s.display !== 'none' && s.opacity !== '0'; };
  const size = sel => { const el = Array.from(document.querySelectorAll(sel)).slice(0, 50).find(shown); return el ? px(getComputedStyle(el).fontSize) : 0; };
  const cv = document.createElement('canvas'); cv.width = cv.height = 1; const g = cv.getContext('2d', {willReadFrequently: true});
  // Computed rgb()/rgba() is read as numbers; a canvas read can be off by one (#154).
  const hex = c => { const h2 = x => x.toString(16).padStart(2, '0'); const m = /^rgba?\((\d+), (\d+), (\d+)(?:, ([\d.]+))?\)$/.exec(c); if (m) { const a = m[4] === undefined ? 1 : +m[4]; return '#' + h2(+m[1]) + h2(+m[2]) + h2(+m[3]) + (a < 1 ? h2(Math.round(a * 255)) : ''); } if (!g) return c; g.clearRect(0, 0, 1, 1); g.fillStyle = '#000'; g.fillStyle = c; g.fillRect(0, 0, 1, 1); const d = g.getImageData(0, 0, 1, 1).data; return '#' + h2(d[0]) + h2(d[1]) + h2(d[2]) + (d[3] < 255 ? h2(d[3]) : ''); };
  // Items side by side: the most children of one box sharing a top edge, counting only children at
  // least 15% of the block's width (cards and columns, not inline links or logo strips).
  const perRow = (root, width) => {
    let best = 1, seen = 0;
    for (const c of [root, ...root.querySelectorAll('*')]) {
      if (++seen > 400) break;
      if (c.children.length < 2) continue;
      const kids = Array.from(c.children).filter(k => shown(k) && k.getBoundingClientRect().width >= Math.max(40, width * 0.15));
      if (kids.length < 2) continue;
      const top = kids[0].getBoundingClientRect().top;
      best = Math.max(best, kids.filter(k => Math.abs(k.getBoundingClientRect().top - top) <= 2).length);
    }
    return best;
  };
  const blocks = [];
  for (const el of document.querySelectorAll('header,nav,main,footer,section,article,aside')) {
    if (blocks.length >= 60) break;
    if (!shown(el)) continue;
    const r = el.getBoundingClientRect(); if (r.height < 24) continue;
    const s = getComputedStyle(el);
    const b = {tag: el.tagName.toLowerCase(), y: Math.round(r.top + scrollY), w: Math.round(r.width), h: Math.round(r.height),
      pad: [px(s.paddingTop), px(s.paddingRight), px(s.paddingBottom), px(s.paddingLeft)], per_row: perRow(el, r.width)};
    const head = Array.from(el.querySelectorAll('h1,h2,h3')).slice(0, 20).find(shown); if (head) b.heading = cap(head.innerText, 80);
    const gap = px(s.columnGap) || px(s.rowGap); if (gap) b.gap = gap;
    if (s.backgroundColor !== 'rgba(0, 0, 0, 0)') b.background = hex(s.backgroundColor);
    // The content column: the first box (itself or inside) held to a maximum width in px.
    let n = 0;
    for (const c of [el, ...el.querySelectorAll('*')]) {
      if (++n > 80) break;
      const cs = getComputedStyle(c);
      if (cs.maxWidth.endsWith('px') && !['IMG', 'SVG', 'VIDEO', 'PICTURE'].includes(c.tagName.toUpperCase()) && shown(c)) {
        const cr = c.getBoundingClientRect();
        b.column = {max_w: px(cs.maxWidth), x: Math.round(cr.left), w: Math.round(cr.width), pad: [px(cs.paddingLeft), px(cs.paddingRight)]};
        break;
      }
    }
    blocks.push(b);
  }
  return {page_w: document.documentElement.scrollWidth, page_h: document.documentElement.scrollHeight,
    type: {h1: size('h1'), h2: size('h2'), h3: size('h3'), body: size('p')}, blocks};
})()"#;

fn web_site_capture(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: CaptureParams = parse_params(params)?;
    if p.widths.is_empty() || p.widths.len() > 4 || p.widths.iter().any(|w| !(320..=3840).contains(w)) {
        return Err(ToolError::invalid("widths must hold 1..=4 values, each 320..=3840"));
    }
    if !(600..=20_000).contains(&p.max_height) {
        return Err(ToolError::invalid("max_height must be 600..=20000"));
    }
    let mut p = p;
    // Clean room keeps what a visitor sees: never the page's own code.
    let visitor = ctx.session.clean_room();
    p.include_html &= !visitor;
    let dl = deadline(ctx, p.timeout_secs.max(30))?;
    let site = open_site(ctx, &dl)?;
    refuse_if_waiting(&site)?;
    let mut client = page_client(&site, &dl)?;
    // A site that didn't open leaves the browser on its own error page (or a blank one), which
    // would be captured as if it were the site (#158).
    let here = eval(&mut client, "location.href", &dl);
    if let Err(e) = &here
        && client.waiting_dialog().is_some()
    {
        return Err(end_with(&site, client, e.clone(), &dl));
    }
    let here = here.ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
    if here.is_empty() || here.starts_with("chrome-error:") || here == "about:blank" {
        cleanup(&mut client, &dl);
        return Err(ToolError::new(ErrorCode::NotFound, "The website didn't open, so there's nothing to capture: the browser is showing its own error page.")
            .with_remediation("Open the site again with web_site_open and make sure it loads."));
    }
    let out = match create_absent_dir(&p.output_directory) {
        Ok(out) => out,
        Err(e) => {
            cleanup(&mut client, &dl);
            return Err(e);
        }
    };
    let r = capture_into(ctx, &mut client, &out, &p, &dl);
    // A question the page asked mid-capture stops it: say what it asks and how to answer it.
    let r = match r {
        Err(e) => return Err(capture_interrupted(end_with(&site, client, e, &dl), &p.output_directory)),
        Ok(r) => r,
    };
    // Leave the page at the first (usually desktop) width for the tools that follow.
    if let Some(w) = p.widths.first() {
        let _ = set_viewport(&mut client, *w, 900, &dl);
    }
    // A question the resize made the page ask is kept for the answer; the next tool says so.
    match client.waiting_dialog().map(dialog_open) {
        Some(open) => hold(&site, open, client),
        None => cleanup(&mut client, &dl),
    }
    let (mut blueprint, shots, html) = r;
    let html_bytes = html.0;
    if visitor {
        visitor_view(&mut blueprint);
    } else if let (Value::Object(b), Ok(h)) = (&mut blueprint, site.health.lock()) {
        b.insert("health".into(), h.clone());
    }
    let signed_in = site.signed_in.is_some();
    let sign_in_page = has_password_field(&blueprint);
    if let Value::Object(b) = &mut blueprint {
        if signed_in && let Some(u) = b.get("url").and_then(Value::as_str).map(shown_url) {
            b.insert("url".into(), json!(u));
        }
        b.insert("signed_in".into(), json!(signed_in));
        b.insert("signed_in_site".into(), json!(site.signed_in));
        b.insert("sign_in_page".into(), json!(sign_in_page));
    }

    let url = blueprint.get("url").and_then(Value::as_str).unwrap_or_default().to_string();
    let shown = shown_url(&url);
    let n = |k: &str| blueprint.get(k).and_then(Value::as_array).map_or(0, Vec::len);
    let images = blueprint.pointer("/assets/images").and_then(Value::as_array).map_or(0, Vec::len);
    let readme = readme(&blueprint, &shots, html, visitor);
    write_new_file(&out, &["README.md".into()], readme.as_bytes()).map_err(ToolError::invalid)?;
    let capture_json = serde_json::to_vec_pretty(&blueprint).map_err(|e| ToolError::internal(e.to_string()))?;
    write_new_file(&out, &["capture.json".into()], &capture_json).map_err(ToolError::invalid)?;

    let summary = format!(
        "Captured {shown}: {} screenshot(s), {} outline items, {} headings, {images} images, {} links, {} forms → {}.",
        shots.len(),
        n("outline"),
        n("headings"),
        n("links"),
        n("forms"),
        out.display()
    );
    let browser = site.handle();
    let ev = EvidenceBuilder::new("web_site_capture", Subject { kind: "website".into(), sha256: None, name: Some(shown.clone()) }, browser_env(ctx.session.clean_room(), &browser))
        .params(json!({"widths": p.widths, "max_height": p.max_height, "include_html": p.include_html, "noise_pass": p.noise_pass, "visitor_view": visitor}))
        .result(json!({
            "output_directory": out.display().to_string(),
            "url": url,
            "title": blueprint.get("title"),
            "screenshots": shots,
            "html_bytes": html_bytes,
            "counts": {"outline": n("outline"), "headings": n("headings"), "links": n("links"), "forms": n("forms"), "images": images},
            "design": blueprint.get("design"),
            "files": ["README.md", "capture.json"],
            "signed_in": signed_in,
            "signed_in_site": site.signed_in,
            "sign_in_page": sign_in_page,
        }))
        .summary(summary)
        .confidence(Confidence::Observed)
        .authority(Authority::OurRun)
        .limitation(Limitation::new("one_state", "One state of the page: menus, dialogs and content behind interactions are not opened."))
        .limitation(if visitor {
            Limitation::new("visitor_view", "Clean room: only what a visitor sees was kept (screenshots, visible text, measured layout and styles); not the page's HTML, CSS rules, scripts, requests or errors.")
        } else {
            Limitation::new("cross_origin_css", "CSS custom properties and breakpoints from cross-origin stylesheets can't be read.")
        })
        .limitations(html_cut(html))
        .limitations(site.signed_in.as_deref().map(|s| profile_limitation(Some(s))))
        .limitations(sign_in_page.then(|| if signed_in { Limitation::new("sign_in_ended", SIGN_IN_ENDED) } else { Limitation::new("sign_in_page", SIGN_IN_HINT) }))
        .coverage(Coverage::complete(p.widths.len() as u64))
        .location(Location::new("url", shown))
        .location(Location::new("path", out.display().to_string()))
        .build()?;
    Ok(ToolOutput::evidence(ev))
}

/// Said when a page captured signed out asks for a password.
const SIGN_IN_HINT: &str =
    "This may be a sign-in page (it asks for a password). To study what's behind it, sign in once with web_site_sign_in and capture again.";
/// Said when a page captured with a saved sign-in still asks for a password.
const SIGN_IN_ENDED: &str = "This was opened with your saved sign-in but may still be a sign-in page (it asks for a password): the site may have ended that sign-in. Sign in again with web_site_sign_in and capture again.";

/// The page asks for a password: a field of type password in its forms or its outline.
fn has_password_field(blueprint: &Value) -> bool {
    let password = |f: &Value| f.get("type").and_then(Value::as_str) == Some("password");
    let in_forms = blueprint
        .get("forms")
        .and_then(Value::as_array)
        .is_some_and(|forms| forms.iter().any(|f| f.get("fields").and_then(Value::as_array).is_some_and(|fields| fields.iter().any(password))));
    in_forms || blueprint.get("outline").and_then(Value::as_array).is_some_and(|items| items.iter().any(password))
}

/// The blueprint, the screenshots, and the HTML's bytes as (saved, the page's).
type Captured = (Value, Vec<Value>, (usize, usize));

/// When page.html holds only the first part of the page's HTML (#336).
fn html_cut((kept, total): (usize, usize)) -> Option<Limitation> {
    (kept < total).then(|| {
        Limitation::new(
            "html_cut",
            format!("page.html holds the first {kept} of the page's {total} bytes of HTML: it was cut at the {} MiB limit.", MAX_HTML_BYTES >> 20),
        )
    })
}

fn capture_into(ctx: &Ctx<'_>, client: &mut CdpClient, out: &Path, p: &CaptureParams, dl: &Deadline) -> Result<Captured, ToolError> {
    client.call("Page.enable", json!({}), None, dl)?;
    let mut shots = Vec::new();
    let mut blueprint = Value::Null;
    let mut layouts = Vec::new();
    for (i, &w) in p.widths.iter().enumerate() {
        ctx.report(Some(i as f32 / p.widths.len() as f32), format!("Photographing the page at {w} px…"));
        set_viewport(client, w, 900, dl)?;
        settle(client, 900)?;
        // A page that can't be scrolled through is still photographed as it is.
        let _ = eval(client, REVEAL_JS, dl);
        if i == 0 {
            // The blueprint is read at the first width, where the layout is the reference one.
            blueprint = eval(client, BLUEPRINT_JS, dl)?;
        }
        // A page whose layout can't be read is still photographed.
        if let Ok(Value::Object(mut layout)) = eval(client, LAYOUT_JS, dl) {
            layout.insert("width".into(), json!(w));
            layouts.push(Value::Object(layout));
        }
        let height = eval(client, "Math.max(document.documentElement.scrollHeight, document.body ? document.body.scrollHeight : 0)", dl)?
            .as_u64()
            .unwrap_or(900)
            .clamp(320, u64::from(p.max_height));
        let clip = json!({"format": "png", "captureBeyondViewport": true, "clip": {"x": 0, "y": 0, "width": w, "height": height, "scale": 1}});
        let photograph = |client: &mut CdpClient| -> Result<Vec<u8>, ToolError> {
            let shot = client.call("Page.captureScreenshot", clip.clone(), None, dl)?;
            let data =
                shot.get("data").and_then(Value::as_str).ok_or_else(|| ToolError::new(ErrorCode::GarbledOutput, "the browser returned no screenshot"))?;
            web_common::b64_decode(data)
        };
        let bytes = photograph(client)?;
        let (pw, ph) = web_common::png_dimensions(&bytes)?;
        let name = format!("screenshot-{w}.png");
        write_new_file(out, std::slice::from_ref(&name), &bytes).map_err(ToolError::invalid)?;
        let mut row = json!({"file": name, "width": pw, "height": ph, "css_width": w, "sha256": sha256_hex(&bytes), "cut": height >= u64::from(p.max_height)});
        if p.noise_pass {
            // The same page twice more, at uneven gaps so something repeating every so often
            // can't look still in both: what changed by itself moves, not the build's fault.
            settle(client, 900)?;
            let second = photograph(client)?;
            settle(client, 1300)?;
            let third = photograph(client)?;
            if let (Ok(a), Ok(b), Ok(c)) = (web_common::decode_png(&bytes), web_common::decode_png(&second), web_common::decode_png(&third)) {
                let mask = web_common::NoiseMask::between(&a, &b, 24).union(&web_common::NoiseMask::between(&a, &c, 24));
                row["noise_tiles"] = json!(mask.count());
                if mask.count() > 0 {
                    let file = format!("noise-{w}.png");
                    write_new_file(out, std::slice::from_ref(&file), &mask.to_png()?).map_err(ToolError::invalid)?;
                    row["noise_file"] = json!(file);
                }
            }
        }
        shots.push(row);
    }
    if let Value::Object(b) = &mut blueprint {
        b.insert("layouts".into(), Value::Array(layouts));
    }
    let mut html_bytes = (0, 0);
    if p.include_html {
        let html = eval(client, "document.documentElement.outerHTML", dl)?;
        let html = html.as_str().unwrap_or_default();
        let kept = truncate_utf8(html, MAX_HTML_BYTES);
        html_bytes = (kept.len(), html.len());
        write_new_file(out, &["page.html".into()], kept.as_bytes()).map_err(ToolError::invalid)?;
    }
    Ok((blueprint, shots, html_bytes))
}

fn truncate_utf8(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s.get(..end).unwrap_or_default()
}

/// Drop everything a visitor can't see: the page's metadata, CSS variables and breakpoints, the
/// stylesheets, scripts, fonts and requests it loaded, where its pictures come from (their sizes
/// stay), its class names, and the query strings (deployment ids, tokens) on the addresses it
/// shows (#156). Screenshots, visible text, the measured layout and styles, links and forms are kept.
fn visitor_view(blueprint: &mut Value) {
    let Value::Object(b) = blueprint else { return };
    for k in ["meta", "breakpoints", "unreadable_stylesheets", "health"] {
        b.remove(k);
    }
    let bare = |v: &mut Value| {
        if let Some(s) = v.as_str() {
            *v = Value::String(s.split(['?', '#']).next().unwrap_or_default().to_string());
        }
    };
    if let Some(Value::Array(items)) = b.get_mut("outline") {
        for item in items.iter_mut().filter_map(Value::as_object_mut) {
            item.remove("class");
            item.get_mut("src").map(bare);
            item.get_mut("href").map(bare);
        }
    }
    if let Some(Value::Array(links)) = b.get_mut("links") {
        links.iter_mut().filter_map(|l| l.get_mut("href")).for_each(bare);
    }
    // "button.btn-primary" → "button": the element, not the page's class for it.
    if let Some(Value::Array(examples)) = b.get_mut("a11y").and_then(|a| a.get_mut("unnamed_examples")) {
        for e in examples.iter_mut() {
            if let Some(s) = e.as_str() {
                *e = Value::String(s.split('.').next().unwrap_or_default().to_string());
            }
        }
    }
    if let Some(Value::Object(d)) = b.get_mut("design") {
        d.remove("css_variables");
        d.remove("font_faces");
    }
    let images: Vec<Value> = b
        .get("assets")
        .and_then(|a| a.get("images"))
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|i| json!({"w": i.get("w"), "h": i.get("h")})).collect())
        .unwrap_or_default();
    b.insert("assets".into(), json!({"images": images}));
}

/// A short human/agent-readable summary of the capture folder.
fn readme(b: &Value, shots: &[Value], (html_bytes, html_total): (usize, usize), visitor: bool) -> String {
    let s = |p: &str| b.pointer(p).and_then(Value::as_str).unwrap_or_default().to_string();
    let top = |p: &str, n: usize| {
        b.pointer(p)
            .and_then(Value::as_array)
            .map(|a| a.iter().take(n).filter_map(|x| x.get("value").and_then(Value::as_str)).map(|v| format!("`{v}`")).collect::<Vec<_>>().join(", "))
            .unwrap_or_default()
    };
    let mut md = format!("# Capture of {}\n\n", shown_url(&s("/url")));
    md.push_str(&format!("- Title: {}\n", s("/title")));
    if !s("/meta/description").is_empty() {
        md.push_str(&format!("- Description: {}\n", s("/meta/description")));
    }
    md.push_str(&format!(
        "- Page size at the first width: {}×{} CSS px\n\n",
        b.pointer("/page/w").and_then(Value::as_u64).unwrap_or(0),
        b.pointer("/page/h").and_then(Value::as_u64).unwrap_or(0)
    ));
    let yes = |k: &str| b.get(k) == Some(&Value::Bool(true));
    if yes("signed_in") && yes("sign_in_page") {
        md.push_str(&format!("{SIGN_IN_ENDED}\n\n"));
    } else if yes("signed_in") {
        md.push_str(&format!(
            "Captured with the person's saved sign-in for {}: if the site still accepted it, this is the site as they see it when signed in, and the pictures and text may show their own account's data.\n\n",
            s("/signed_in_site")
        ));
    } else if yes("sign_in_page") {
        md.push_str(&format!("{SIGN_IN_HINT}\n\n"));
    }
    md.push_str("## Files\n\n");
    for sh in shots {
        md.push_str(&format!(
            "- `{}` — {}×{} px screenshot{}\n",
            sh["file"].as_str().unwrap_or_default(),
            sh["width"],
            sh["height"],
            if sh["cut"].as_bool().unwrap_or(false) { " (cut at the height limit)" } else { "" }
        ));
        if let Some(noise) = sh["noise_file"].as_str() {
            md.push_str(&format!(
                "- `{noise}` — the parts that changed by themselves between two photos a moment apart (white, one pixel per 16 px square): carousels, videos, animations. They aren't compared.\n"
            ));
        }
    }
    if html_bytes > 0 {
        let cut = if html_bytes < html_total { format!(": the first part of {html_total}, cut at the size limit") } else { String::new() };
        md.push_str(&format!("- `page.html` — rendered HTML ({html_bytes} bytes{cut})\n"));
    }
    if visitor {
        md.push_str("- `capture.json` — design tokens, the layout at each width (`layouts`), layout outline (positions in CSS px), headings, links, forms, picture sizes and visible text\n\n");
        md.push_str("Clean room: only what a visitor sees was kept. The page's HTML, CSS rules, scripts, requests and errors weren't read.\n\n");
    } else {
        md.push_str("- `capture.json` — design tokens, breakpoints, the layout at each width (`layouts`), layout outline (positions in CSS px), headings, links, forms, assets and visible text\n\n");
    }
    md.push_str("## Design at a glance\n\n");
    md.push_str(&format!("- Fonts: {}\n", top("/design/fonts", 4)));
    md.push_str(&format!("- Text sizes: {}\n", top("/design/font_sizes", 8)));
    md.push_str(&format!("- Text colours: {}\n", top("/design/text_colors", 6)));
    md.push_str(&format!("- Backgrounds: {}\n", top("/design/backgrounds", 6)));
    md.push_str(&format!("- Corner radii: {}\n", top("/design/radii", 5)));
    md.push_str(&format!("- Spacing (padding, gaps, margins): {}\n", top("/design/spacing", 10)));
    md.push_str(&format!("- Content widths: {}\n", top("/design/max_widths", 4)));
    if visitor {
        md.push('\n');
    } else {
        md.push_str(&format!("- Font files: {}\n\n", b.pointer("/assets/fonts").and_then(Value::as_array).map_or(0, Vec::len)));
    }
    md.push_str(&layout_section(b, visitor));
    if !visitor {
        md.push_str(&health_section(b));
    }
    md.push_str("## Headings\n\n");
    for h in b.pointer("/headings").and_then(Value::as_array).into_iter().flatten().take(40) {
        let level = h.get("level").and_then(Value::as_u64).unwrap_or(1) as usize;
        md.push_str(&format!("{}- {}\n", "  ".repeat(level.saturating_sub(1)), h.get("text").and_then(Value::as_str).unwrap_or_default()));
    }
    md
}

/// What went wrong while the page loaded and how it reads to a screen reader, for the README.
fn health_section(b: &Value) -> String {
    let list = |p: &str| b.pointer(p).and_then(Value::as_array).cloned().unwrap_or_default();
    let (errors, failed) = (list("/health/errors"), list("/health/failed_requests"));
    let mut md = String::from("## Page health\n\n");
    if errors.is_empty() && failed.is_empty() {
        md.push_str("It loaded without script errors or failed requests.\n\n");
    } else {
        for e in errors.iter().take(10) {
            md.push_str(&format!("- {} error: {}\n", e["kind"].as_str().unwrap_or("script"), e["text"].as_str().unwrap_or_default()));
        }
        for f in failed.iter().take(10) {
            let why = f["status"].as_u64().map(|s| s.to_string()).or_else(|| f["error"].as_str().map(String::from)).unwrap_or_default();
            md.push_str(&format!("- failed request ({why}): {}\n", f["url"].as_str().unwrap_or_default()));
        }
        md.push('\n');
    }
    if let Some(a) = b.get("a11y") {
        let n = |k: &str| a.get(k).and_then(Value::as_u64).unwrap_or(0);
        let lang = a.get("lang").and_then(Value::as_str).filter(|l| !l.is_empty()).unwrap_or("not set");
        md.push_str(&format!(
            "## Accessibility\n\nLanguage: {lang}. Controls without a name: {}. Pictures without alt text: {}.\n\n",
            n("unnamed_controls"),
            n("images_without_alt")
        ));
    }
    md
}

/// The widths the page's CSS switches layout at, for the README.
fn breakpoints_section(b: &Value) -> String {
    let bps: Vec<String> = b
        .pointer("/breakpoints")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|q| q.get("query").and_then(Value::as_str))
        .map(String::from)
        .collect();
    let unreadable = b.get("unreadable_stylesheets").and_then(Value::as_u64).unwrap_or(0);
    let body = match (bps.is_empty(), unreadable) {
        (true, 0) => "Its CSS has no width breakpoints.\n\n".to_string(),
        (true, u) => format!("None readable: {u} stylesheet(s) from other sites can't be read; compare the screenshots at each width.\n\n"),
        (false, _) => format!("Its CSS switches layout at: {}\n\n", bps.join(", ")),
    };
    format!("## Breakpoints\n\n{body}")
}

/// Breakpoints (not in a visitor's view) and the layout at each width, for the README.
fn layout_section(b: &Value, visitor: bool) -> String {
    let n = |v: &Value, k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
    let mut md = String::new();
    if !visitor {
        md.push_str(&breakpoints_section(b));
    }
    md.push_str("## Layout at each width\n\n");
    for l in b.pointer("/layouts").and_then(Value::as_array).into_iter().flatten() {
        let (w, pw) = (n(l, "width"), n(l, "page_w"));
        let ty = |k: &str| l.pointer(&format!("/type/{k}")).and_then(Value::as_u64).unwrap_or(0);
        let scroll = if pw > w + 1 { format!("page {pw} px wide, so it scrolls sideways") } else { "no sideways scroll".into() };
        md.push_str(&format!("### {w} px ({scroll})\n\nHeadings {}/{}/{} px, text {} px.\n\n", ty("h1"), ty("h2"), ty("h3"), ty("body")));
        for blk in l.get("blocks").and_then(Value::as_array).into_iter().flatten().take(16) {
            md.push_str(&format!("- {}\n", block_line(blk)));
        }
        md.push('\n');
    }
    md
}

/// One section at one width: `section "Pricing": padding 96 32 96 32, gap 24, 3 per row, column 1280 px at x 80`.
fn block_line(blk: &Value) -> String {
    let n = |v: &Value, k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
    let mut s = blk.get("tag").and_then(Value::as_str).unwrap_or("block").to_string();
    if let Some(h) = blk.get("heading").and_then(Value::as_str) {
        s.push_str(&format!(" \"{h}\""));
    }
    let pad: Vec<String> = blk.get("pad").and_then(Value::as_array).into_iter().flatten().map(|p| p.as_u64().unwrap_or(0).to_string()).collect();
    s.push_str(&format!(": padding {}", pad.join(" ")));
    if n(blk, "gap") > 0 {
        s.push_str(&format!(", gap {}", n(blk, "gap")));
    }
    if n(blk, "per_row") > 1 {
        s.push_str(&format!(", {} per row", n(blk, "per_row")));
    }
    if let Some(c) = blk.get("column") {
        s.push_str(&format!(", column {} px at x {}", n(c, "max_w"), n(c, "x")));
    }
    if let Some(bg) = blk.get("background").and_then(Value::as_str) {
        s.push_str(&format!(", background {bg}"));
    }
    s
}

// ---------------------------------------------------------------------------------------------
// web_site_close

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CloseParams {}

fn web_site_close(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let _: CloseParams = parse_params(params)?;
    let closed = ctx.session.ext.remove::<SiteBrowser>().is_some();
    Ok(ToolOutput::Value(json!({"closed": closed})))
}

#[cfg(test)]
mod tests {
    /// Reuse requires the same saved key and revision, and never an explicit fresh profile.
    #[test]
    fn profile_reuse_requires_current_revision_and_no_explicit_fresh_request() {
        let saved = super::site_sign_in::SavedSignIn {
            key: "site-example.com_443".into(),
            profile: std::path::PathBuf::from("profile"),
            saved_at: 1,
            revision: "second".into(),
        };
        for requested in [None, Some(true)] {
            assert!(super::reuse_profile(requested, Some(&saved.key), Some("second"), Some(&saved)));
            assert!(!super::reuse_profile(requested, Some(&saved.key), Some("first"), Some(&saved)));
            assert!(!super::reuse_profile(requested, Some("site-other.example_443"), Some("second"), Some(&saved)));
            assert!(!super::reuse_profile(requested, None, None, Some(&saved)));
            assert!(!super::reuse_profile(requested, Some(&saved.key), Some("second"), None));
        }
        assert!(super::reuse_profile(None, None, None, None));
        assert!(!super::reuse_profile(Some(false), None, None, None));
        assert!(!super::reuse_profile(Some(false), Some(&saved.key), Some("second"), Some(&saved)));
    }

    use super::*;

    #[test]
    fn a_click_is_described_by_what_changed() {
        let before = json!({"expanded": "false", "selected": null, "open": null, "dialogs": 0, "url": "https://a.test/"});
        let menu = json!({"expanded": "true", "selected": null, "open": null, "dialogs": 0, "url": "https://a.test/"});
        assert_eq!(state_change(&before, &menu), vec!["expanded"]);
        let dialog = json!({"expanded": "false", "selected": null, "open": null, "dialogs": 1, "url": "https://a.test/"});
        assert_eq!(state_change(&before, &dialog), vec!["dialog appeared"]);
        let away = json!({"expanded": "false", "selected": null, "open": null, "dialogs": 0, "url": "https://a.test/other"});
        assert_eq!(state_change(&before, &away), vec!["address changed"]);
        let tab = json!({"selected": "true", "url": "https://a.test/"});
        assert_eq!(state_change(&json!({"selected": "false", "url": "https://a.test/"}), &tab), vec!["tab selected"]);
        assert!(state_change(&before, &before).is_empty());
        // The page scripts carry their placeholders.
        assert!(FIND_STATES_JS.contains("__MAX__") && STATE_JS.contains("__I__") && CLICK_STATE_JS.contains("__I__"));
        assert!(REMEMBER_TAB_JS.contains("__I__") && RESELECT_TAB_JS.contains("__I__"));
    }

    #[test]
    fn urls_are_checked() {
        assert!(check_url("https://example.com").is_ok());
        assert!(check_url(" http://a.b/c?d ").is_ok());
        for bad in ["example.com", "file:///etc/passwd", "javascript:alert(1)", "https://", "https:///x", "https://a b.com", "chrome://settings"] {
            assert!(check_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn truncation_keeps_char_boundaries() {
        assert_eq!(truncate_utf8("héllo", 2), "h");
        assert_eq!(truncate_utf8("abc", 10), "abc");
    }

    #[test]
    fn readme_lists_files_and_tokens() {
        let b = json!({"url": "https://ex.com/a?secret=1", "title": "Ex", "page": {"w": 1440, "h": 3000},
            "design": {"fonts": [{"value": "Inter", "count": 9}], "font_sizes": [], "text_colors": [], "backgrounds": [], "radii": []},
            "headings": [{"level": 1, "text": "Hello"}, {"level": 2, "text": "World"}]});
        let md = readme(&b, &[json!({"file": "screenshot-1440.png", "width": 1440, "height": 3000, "cut": false})], (10, 10), false);
        assert!(md.contains("screenshot-1440.png") && md.contains("`Inter`") && md.contains("  - World") && md.contains("page.html"));
        assert!(!md.contains("secret=1"), "query strings are not echoed");
        assert!(md.contains("no width breakpoints"), "{md}");
        assert!(!md.contains("cut at the size limit") && html_cut((10, 10)).is_none());
    }

    /// #1178: a page captured signed out that asks for a password suggests signing in; a signed-in
    /// capture says it shows the person's own view, and never suggests it.
    #[test]
    fn a_sign_in_page_and_a_signed_in_capture_are_said() {
        let login = json!({"forms": [{"fields": [{"tag": "input", "type": "email"}, {"tag": "input", "type": "password"}]}]});
        assert!(has_password_field(&login));
        assert!(has_password_field(&json!({"outline": [{"tag": "input", "type": "password"}]})));
        assert!(!has_password_field(&json!({"forms": [{"fields": [{"type": "search"}]}], "outline": [{"tag": "a"}]})));
        assert!(!has_password_field(&json!({"forms": "garbage", "outline": 3})));
        let page = |extra: Value| {
            let mut b = json!({"url": "https://ex.com/", "title": "Ex", "page": {"w": 1440, "h": 900}});
            if let (Value::Object(b), Value::Object(e)) = (&mut b, extra) {
                b.extend(e);
            }
            readme(&b, &[], (0, 0), true)
        };
        assert!(page(json!({"signed_in": false, "sign_in_page": true})).contains(SIGN_IN_HINT));
        let signed = page(json!({"signed_in": true, "signed_in_site": "ex.com", "sign_in_page": false}));
        assert!(signed.contains("Captured with the person's saved sign-in") && signed.contains("ex.com") && !signed.contains(SIGN_IN_HINT), "{signed}");
        assert!(!page(json!({})).contains("signed in"));
        let ended = page(json!({"signed_in": true, "signed_in_site": "ex.com", "sign_in_page": true}));
        assert!(ended.contains(SIGN_IN_ENDED) && !ended.contains("Captured with the person's saved sign-in"), "{ended}");
        let (u, f) = signed_in_urls("https://ex.com/inbox?code=abc#t".into(), json!({"url": "https://ex.com/inbox?code=abc", "title": "x"}));
        assert_eq!((u.as_str(), f["url"].as_str()), ("https://ex.com/inbox", Some("https://ex.com/inbox")));
        assert!(profile_limitation(Some("ex.com")).message.contains("your own account"));
        assert_eq!(profile_limitation(None).code, "private_profile");
    }

    /// #336: HTML cut at the size limit is said in the evidence and the README.
    #[test]
    fn html_cut_at_the_limit_is_said() {
        let md = readme(&json!({}), &[], (MAX_HTML_BYTES, MAX_HTML_BYTES + 1174), false);
        assert!(md.contains(&format!("rendered HTML ({MAX_HTML_BYTES} bytes: the first part of {}, cut at the size limit)", MAX_HTML_BYTES + 1174)), "{md}");
        let l = html_cut((MAX_HTML_BYTES, MAX_HTML_BYTES + 1174)).unwrap();
        assert_eq!(l.code, "html_cut");
        assert!(l.message.contains("4 MiB"), "{}", l.message);
        assert!(html_cut((0, 0)).is_none());
    }

    #[test]
    fn page_health_keeps_errors_and_failed_requests_without_query_strings() {
        let ev = |method: &str, params: Value| CdpEvent { method: method.into(), params, session_id: None };
        let events = vec![
            ev("Network.requestWillBeSent", json!({"requestId": "7", "request": {"url": "https://ex.com/font.woff2?token=abc"}})),
            ev(
                "Runtime.exceptionThrown",
                json!({"exceptionDetails": {"text": "Uncaught", "url": "https://ex.com/app.js?v=2", "exception": {"description": "TypeError: x is undefined\n    at app.js:1"}}}),
            ),
            ev(
                "Runtime.exceptionThrown",
                json!({"exceptionDetails": {"text": "Uncaught", "exception": {"description": "TypeError: x is undefined\n    at app.js:9"}}}),
            ),
            ev(
                "Runtime.consoleAPICalled",
                json!({"type": "error", "args": [{"type": "string", "value": "Hydration failed"}, {"type": "object", "description": "Error"}]}),
            ),
            ev("Runtime.consoleAPICalled", json!({"type": "log", "args": [{"type": "string", "value": "hello"}]})),
            ev("Log.entryAdded", json!({"entry": {"level": "error", "source": "network", "text": "Failed to load resource: 404"}})),
            ev("Log.entryAdded", json!({"entry": {"level": "error", "source": "security", "text": "Mixed content blocked", "url": "https://ex.com/"}})),
            ev("Network.responseReceived", json!({"type": "Image", "response": {"url": "https://ex.com/logo.png?sig=1", "status": 404}})),
            ev("Network.responseReceived", json!({"type": "Document", "response": {"url": "https://ex.com/", "status": 200}})),
            ev("Network.loadingFailed", json!({"requestId": "7", "type": "Font", "errorText": "net::ERR_NAME_NOT_RESOLVED"})),
            ev("Network.loadingFailed", json!({"requestId": "8", "type": "Media", "errorText": "net::ERR_ABORTED", "canceled": true})),
        ];
        let h = page_health(&events, false);
        let texts: Vec<&str> = h["errors"].as_array().unwrap().iter().map(|e| e["text"].as_str().unwrap()).collect();
        assert_eq!(texts, ["TypeError: x is undefined", "Hydration failed Error", "Mixed content blocked"], "one line each, once, no network log noise");
        assert_eq!(h["errors"][0]["url"], "https://ex.com/app.js");
        let failed = h["failed_requests"].as_array().unwrap();
        assert_eq!(failed.len(), 2, "a 404 and a failed font; the cancelled load and the 200 aren't failures");
        assert_eq!(failed[0], json!({"url": "https://ex.com/logo.png", "status": 404, "type": "Image"}));
        assert_eq!(failed[1]["url"], "https://ex.com/font.woff2");
        assert!(!h.to_string().contains("token") && !h.to_string().contains("sig="), "no query strings");
        assert_eq!(page_health(&[], false), json!({"errors": [], "failed_requests": []}));
        let md = readme(&json!({"health": h, "a11y": {"lang": "en", "unnamed_controls": 2, "images_without_alt": 1}}), &[], (0, 0), false);
        assert!(md.contains("- script error: TypeError: x is undefined") && md.contains("- failed request (404): https://ex.com/logo.png"), "{md}");
        assert!(md.contains("Language: en. Controls without a name: 2. Pictures without alt text: 1."), "{md}");
        assert!(readme(&json!({"health": page_health(&[], false)}), &[], (0, 0), false).contains("loaded without script errors"));
    }

    #[test]
    fn dialogs_a_page_opened_are_kept_with_their_answer_and_said() {
        let ev = |params: Value| CdpEvent { method: cdp::DIALOG_OPENING.into(), params, session_id: None };
        let events = [
            ev(json!({"type": "alert", "message": "Settings changed; resetting.\nline two", "url": "https://ex.com/?k=1"})),
            ev(json!({"type": "confirm", "message": "Delete everything?"})),
        ];
        let h = page_health(&events, false);
        assert_eq!(
            h["dialogs"],
            json!([
                {"type": "alert", "message": "Settings changed; resetting.", "answered": "accepted"},
                {"type": "confirm", "message": "Delete everything?", "answered": "dismissed"},
            ]),
            "one line each, and a confirm our own click opened is never agreed to"
        );
        assert!(page_health(&[], false).get("dialogs").is_none(), "a page without dialogs keeps the old shape");
        let said = dialogs_said(&h["dialogs"]);
        assert_eq!(said, " The page opened an alert by itself (\"Settings changed; resetting.\") and 1 more; it was accepted so the page could go on.");
        assert_eq!(dialogs_said(&json!([])), "");
        assert_eq!(dialogs_said(&Value::Null), "");
        // Held for the agent: the question is waiting, and only the alert is said as answered.
        let held = page_health(&events, true);
        assert_eq!(held["dialogs"][1]["answered"], "waiting");
        assert_eq!(
            dialogs_said(&held["dialogs"]),
            " The page opened an alert by itself (\"Settings changed; resetting.\"); it was accepted so the page could go on."
        );
    }

    #[test]
    fn a_question_the_page_waits_on_is_said_with_how_to_answer_it() {
        let open =
            dialog_open(&json!({"type": "confirm", "message": "Continue to the new version?\nsecond line", "defaultPrompt": "", "url": "https://ex.com/?t=1"}));
        assert_eq!(open, json!({"type": "confirm", "message": "Continue to the new version?"}), "first line only, no address");
        let prompt = dialog_open(&json!({"type": "prompt", "message": "Your team?", "defaultPrompt": "Sales"}));
        assert_eq!(prompt["default_prompt"], "Sales");
        assert_eq!(
            dialog_waiting_said(&open),
            " The page is waiting for an answer to a confirm question (\"Continue to the new version?\"): nothing in it can be read until it's answered. Answer it with web_page_dialog_answer (accept: true agrees, false dismisses)."
        );
        // A tool the question stops says what it asks and names the tool that answers it.
        let e = dialog_refusal(&prompt);
        assert_eq!(e.message, "The page is waiting for an answer to its prompt (\"Your team?\"), so nothing in it can be read.");
        assert!(e.remediation.as_deref().is_some_and(|r| r.contains(DIALOG_ANSWER_TOOL)), "{e:?}");
        assert_eq!(e.details.as_ref().map(|d| d[cdp::DIALOG_DETAIL].clone()), Some(prompt));
    }

    /// A capture after an answer records the answer and what the page reported after it, not the
    /// health from before with the question still waiting.
    #[test]
    fn the_stored_health_takes_the_answer_and_what_followed() {
        let stored = json!({
            "errors": [{"kind": "console", "text": "old", "url": ""}],
            "failed_requests": [],
            "dialogs": [{"type": "alert", "message": "Hi", "answered": "accepted"}, {"type": "confirm", "message": "Continue?", "answered": "waiting"}],
            "dialog_open": {"type": "confirm", "message": "Continue?"},
        });
        let after = json!({
            "errors": [{"kind": "console", "text": "old", "url": ""}, {"kind": "console", "text": "after the answer", "url": ""}],
            "failed_requests": [{"url": "https://ex.com/api", "status": 500, "type": "XHR"}],
            "dialogs": [{"type": "prompt", "message": "Your team?", "answered": "waiting"}],
            "dialog_open": {"type": "prompt", "message": "Your team?", "default_prompt": "Sales"},
        });
        let h = health_after_answer(&stored, true, &after);
        assert_eq!(h["dialogs"][1]["answered"], "accepted", "{h}");
        assert_eq!(h["dialogs"][2]["message"], "Your team?");
        assert_eq!(h["errors"].as_array().map(Vec::len), Some(2), "each text once: {h}");
        assert_eq!(h["errors"][1]["text"], "after the answer");
        assert_eq!(h["failed_requests"][0]["status"], 500);
        assert_eq!(h["dialog_open"]["type"], "prompt", "the next question is the one open");

        let quiet = health_after_answer(&stored, false, &json!({"errors": [], "failed_requests": []}));
        assert_eq!(quiet["dialogs"][1]["answered"], "dismissed");
        assert!(quiet.get("dialog_open").is_none(), "nothing open after the answer: {quiet}");
        // An answer whose follow-up couldn't be read still settles the question.
        let unread = health_after_answer(&stored, true, &Value::Null);
        assert_eq!(unread["dialogs"][1]["answered"], "accepted");
        assert!(unread.get("dialog_open").is_none());
        assert_eq!(health_after_answer(&Value::Null, true, &after)["errors"].as_array().map(Vec::len), Some(2), "nothing stored before");
    }

    /// A page connection to a stand-in browser that runs `script` on its WebSocket and answers
    /// `/json/version` (so the open website counts as live); no browser starts.
    fn stand_in_page(script: impl FnOnce(&mut std::net::TcpStream) + Send + 'static) -> (CdpClient, Endpoint) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut script = Some(script);
        std::thread::spawn(move || {
            for mut s in listener.incoming().flatten() {
                let mut head = Vec::new();
                let mut b = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") && s.read(&mut b).unwrap_or(0) == 1 {
                    head.push(b[0]);
                }
                let text = String::from_utf8_lossy(&head).into_owned();
                let Some(key) = text.lines().find_map(|l| l.strip_prefix("Sec-WebSocket-Key: ")).map(|k| k.trim().to_string()) else {
                    let body = json!({"Browser": "Stand-in/1", "Protocol-Version": "1.3"}).to_string();
                    let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    continue;
                };
                let accept = {
                    use base64::Engine as _;
                    use sha1::{Digest, Sha1};
                    let mut h = Sha1::new();
                    h.update(key.as_bytes());
                    h.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
                    base64::engine::general_purpose::STANDARD.encode(h.finalize())
                };
                let reply = format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n");
                s.write_all(reply.as_bytes()).unwrap();
                s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
                if let Some(script) = script.take() {
                    std::thread::spawn(move || {
                        script(&mut s);
                        // Stay connected until the client is done.
                        let _ = s.read(&mut [0u8; 1]);
                    });
                }
            }
        });
        let ep = cdp::parse_endpoint(&format!("http://127.0.0.1:{port}")).unwrap();
        let target = cdp::ws_target(&format!("ws://127.0.0.1:{port}/devtools/page/P"), &ep).unwrap();
        let dl = Deadline::new(Duration::from_secs(5), crate::tool::CancelToken::default());
        (CdpClient::new(cdp::WsConn::connect(&target, 1 << 20, &dl).unwrap()).answering_notices(), ep)
    }

    /// Send one unmasked text frame, as a browser does.
    fn send_frame(s: &mut std::net::TcpStream, v: Value) {
        use std::io::Write;
        let body = v.to_string().into_bytes();
        let mut frame = vec![0x81u8];
        match u8::try_from(body.len()) {
            Ok(n) if n < 126 => frame.push(n),
            _ => {
                frame.push(126);
                frame.extend_from_slice(&u16::try_from(body.len()).unwrap().to_be_bytes());
            }
        }
        frame.extend_from_slice(&body);
        s.write_all(&frame).unwrap();
    }

    /// Read one masked client frame's JSON; `None` once the client closes.
    fn read_frame(s: &mut std::net::TcpStream) -> Option<Value> {
        use std::io::Read;
        let mut hdr = [0u8; 2];
        s.read_exact(&mut hdr).ok()?;
        let mut len = usize::from(hdr[1] & 0x7f);
        if len == 126 {
            let mut e = [0u8; 2];
            s.read_exact(&mut e).ok()?;
            len = usize::from(u16::from_be_bytes(e));
        }
        let mut mask = [0u8; 4];
        s.read_exact(&mut mask).ok()?;
        let mut body = vec![0u8; len];
        s.read_exact(&mut body).ok()?;
        for (i, b) in body.iter_mut().enumerate() {
            *b ^= mask[i % 4];
        }
        if hdr[0] & 0x0f == 0x8 {
            return None;
        }
        serde_json::from_slice(&body).ok()
    }

    /// Reply to the client's commands as a page would (its facts for an evaluate, `{}` otherwise)
    /// until one `hand_back` picks, which is returned unanswered; `None` once the client closes.
    fn reply_until(s: &mut std::net::TcpStream, hand_back: impl Fn(&Value) -> bool) -> Option<Value> {
        loop {
            let m = read_frame(s)?;
            if hand_back(&m) {
                return Some(m);
            }
            let result = if m["method"] == "Runtime.evaluate" {
                json!({"result": {"type": "object", "value": {"url": "http://page.test/done", "title": "Done", "ready": "complete"}}})
            } else {
                json!({})
            };
            send_frame(s, json!({"id": m["id"], "result": result}));
        }
    }

    /// An engine whose open website is the stand-in page `client` reaches, waiting on the
    /// question it has already sent.
    fn engine_held(client: CdpClient, ep: &Endpoint) -> crate::Engine {
        engine_held_as(client, ep, true)
    }

    /// [`engine_held`]; `read` false leaves the question's opening in the client's queue, as a
    /// question that stopped a capture's own reads is held.
    fn engine_held_as(mut client: CdpClient, ep: &Endpoint, read: bool) -> crate::Engine {
        client.pump_until(&Deadline::new(Duration::from_millis(500), crate::tool::CancelToken::default())).unwrap();
        let asking = client.waiting_dialog().expect("the stand-in asked a question").clone();
        let events = if read { client.take_events() } else { vec![CdpEvent { method: cdp::DIALOG_OPENING.into(), params: asking.clone(), session_id: None }] };
        let open = dialog_open(&asking);
        let mut health = page_health(&events, true);
        health["dialog_open"] = open.clone();
        let site = SiteBrowser {
            endpoint: ep.clone(),
            product: "Stand-in/1".into(),
            version: "1.3".into(),
            url: Mutex::new("http://page.test/".into()),
            health: Mutex::new(health),
            held: Mutex::new(Some(HeldDialog { open, client })),
            signed_in: None,
            sign_in_revision: None,
            _owner: Mutex::new(Browser::attached(ep, "Stand-in/1", "1.3")),
        };
        let mut s = crate::testing::session();
        // The tool asks for a browser before it runs; this stand-in file is never started.
        s.settings.engines.tools.chromium = Some(std::env::current_exe().unwrap());
        s.ext.insert(Arc::new(site));
        crate::Engine::new(s)
    }

    fn answer(e: &crate::Engine, params: Value) -> Result<Value, ToolError> {
        let opts = crate::CallOptions::new(crate::Caller::Mcp).with_consent(crate::Consent::Granted);
        e.call(DIALOG_ANSWER_TOOL, params, opts).map(|o| o.to_json())
    }

    fn stored_health(e: &crate::Engine) -> Value {
        let site = e.session().ext.get::<SiteBrowser>().unwrap();
        site.health.lock().unwrap().clone()
    }

    /// #2019 review: a question the page asks before an answer's reply is recorded once, as the
    /// one waiting, and once answered the stored health says so instead of listing it again as
    /// something the page did after the answer.
    #[test]
    fn a_second_question_is_recorded_once_and_answered() {
        let (client, ep) = stand_in_page(|s| {
            send_frame(s, json!({"method": cdp::DIALOG_OPENING, "params": {"type": "confirm", "message": "Continue?"}}));
            // The first answer: the page asks next before its reply.
            let first = read_frame(s).unwrap();
            assert_eq!(first["method"], "Page.handleJavaScriptDialog");
            send_frame(s, json!({"method": cdp::DIALOG_OPENING, "params": {"type": "prompt", "message": "Your team?", "defaultPrompt": "Sales"}}));
            send_frame(s, json!({"id": first["id"], "result": {}}));
            // The second answer, then the page goes on.
            let second = reply_until(s, |m| m["method"] == "Page.handleJavaScriptDialog").unwrap();
            assert_eq!(second["params"], json!({"accept": true, "promptText": "Design"}));
            send_frame(s, json!({"id": second["id"], "result": {}}));
            send_frame(s, json!({"method": cdp::DIALOG_CLOSED, "params": {"result": true}}));
            reply_until(s, |_| false);
        });
        let e = engine_held(client, &ep);
        let asked = answer(&e, json!({"accept": true, "settle_ms": 100})).unwrap();
        assert_eq!(asked["result"]["dialog_open"]["type"], "prompt", "{asked}");
        let h = stored_health(&e);
        assert_eq!(
            h["dialogs"],
            json!([{"type": "confirm", "message": "Continue?", "answered": "accepted"}, {"type": "prompt", "message": "Your team?", "answered": "waiting"}]),
            "{h}"
        );
        let done = answer(&e, json!({"accept": true, "prompt_text": "Design", "settle_ms": 100})).unwrap();
        assert_eq!(done["result"]["title"], "Done", "{done}");
        let h = stored_health(&e);
        assert_eq!(
            h["dialogs"],
            json!([{"type": "confirm", "message": "Continue?", "answered": "accepted"}, {"type": "prompt", "message": "Your team?", "answered": "accepted"}]),
            "{h}"
        );
        assert!(h.get("dialog_open").is_none(), "{h}");
    }

    /// #2019 review: leaving a page dismisses a question asked in reply to the dismissal too,
    /// instead of closing the only connection that could answer it.
    #[test]
    fn leaving_dismisses_a_question_the_dismissal_raised() {
        let (tx, rx) = std::sync::mpsc::channel();
        let (mut client, _) = stand_in_page(move |s| {
            send_frame(s, json!({"method": cdp::DIALOG_OPENING, "params": {"type": "confirm", "message": "Leave?"}}));
            let first = read_frame(s).unwrap();
            // Cancel makes the page ask again, before the reply.
            send_frame(s, json!({"method": cdp::DIALOG_OPENING, "params": {"type": "confirm", "message": "Really?"}}));
            send_frame(s, json!({"id": first["id"], "result": {}}));
            let answers: Vec<Value> = std::iter::from_fn(|| reply_until(s, |m| m["method"] == "Page.handleJavaScriptDialog")).collect();
            let _ = tx.send((first, answers));
        });
        client.pump_until(&Deadline::new(Duration::from_millis(500), crate::tool::CancelToken::default())).unwrap();
        let open = dialog_open(client.waiting_dialog().unwrap());
        dismiss(HeldDialog { open, client });
        let (first, answers) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(first["params"], json!({"accept": false}));
        assert_eq!(answers.iter().map(|a| a["params"].clone()).collect::<Vec<_>>(), vec![json!({"accept": false})], "the second question is dismissed too");
    }

    /// #2019 review: when the question closes before the answer's reply and the reply is late, the
    /// connection is kept and the next try reads what the answer did.
    #[test]
    fn an_answer_whose_question_closed_first_is_still_read() {
        let (client, ep) = stand_in_page(|s| {
            send_frame(s, json!({"method": cdp::DIALOG_OPENING, "params": {"type": "confirm", "message": "Continue?"}}));
            let first = read_frame(s).unwrap();
            send_frame(s, json!({"method": cdp::DIALOG_CLOSED, "params": {"result": true}}));
            std::thread::sleep(Duration::from_millis(1500));
            send_frame(s, json!({"id": first["id"], "result": {}}));
            reply_until(s, |_| false);
        });
        let e = engine_held(client, &ep);
        let late = answer(&e, json!({"accept": true, "timeout_secs": 1})).unwrap_err();
        assert_eq!(late.code, ErrorCode::TimedOut, "kept for another try, not a lost connection: {late:?}");
        let read = answer(&e, json!({"accept": false, "settle_ms": 100})).unwrap();
        assert_eq!(read["result"]["answered"], json!({"dialog": {"type": "confirm", "message": "Continue?"}, "accept": true, "earlier": true}), "{read}");
        assert_eq!(read["result"]["title"], "Done");
    }

    /// #2019 review: what the page reported while the answer and setup awaited replies is kept,
    /// even with no settle time.
    #[test]
    fn events_before_the_settle_are_kept_with_no_settle_time() {
        let (client, ep) = stand_in_page(|s| {
            send_frame(s, json!({"method": cdp::DIALOG_OPENING, "params": {"type": "confirm", "message": "Continue?"}}));
            let first = read_frame(s).unwrap();
            send_frame(
                s,
                json!({"method": "Runtime.consoleAPICalled", "params": {"type": "error", "args": [{"type": "string", "value": "after the answer"}]}}),
            );
            send_frame(s, json!({"id": first["id"], "result": {}}));
            reply_until(s, |_| false);
        });
        let e = engine_held(client, &ep);
        answer(&e, json!({"accept": true, "settle_ms": 0})).unwrap();
        let h = stored_health(&e);
        assert!(h["errors"].as_array().unwrap().iter().any(|x| x["text"] == "after the answer"), "{h}");
    }

    /// #2019 review: a capture a question stopped says to run again into a new folder, since its
    /// own folder already holds part of the capture; other errors keep their own advice.
    #[test]
    fn an_interrupted_capture_is_retried_in_a_new_folder() {
        let asked = capture_interrupted(dialog_refusal(&json!({"type": "confirm", "message": "Continue?"})), "out/capture");
        let fix = asked.remediation.unwrap_or_default();
        assert!(fix.contains(DIALOG_ANSWER_TOOL) && fix.contains("new output_directory") && fix.contains("out/capture"), "{fix}");
        let other = ToolError::new(ErrorCode::ProgramFailed, "screenshot failed").with_remediation("Try again.");
        assert_eq!(capture_interrupted(other, "out/capture").remediation.as_deref(), Some("Try again."));
    }

    /// #2019 review: the held question's own opening, still queued (a capture held it), isn't
    /// listed again as something the page did after the answer.
    #[test]
    fn the_answered_question_is_listed_once() {
        let (client, ep) = stand_in_page(|s| {
            send_frame(s, json!({"method": cdp::DIALOG_OPENING, "params": {"type": "confirm", "message": "Continue?"}}));
            send_frame(
                s,
                json!({"method": "Runtime.consoleAPICalled", "params": {"type": "error", "args": [{"type": "string", "value": "before the answer"}]}}),
            );
            let first = read_frame(s).unwrap();
            send_frame(s, json!({"id": first["id"], "result": {}}));
            reply_until(s, |_| false);
        });
        let e = engine_held_as(client, &ep, false);
        let done = answer(&e, json!({"accept": true, "settle_ms": 100})).unwrap();
        assert_eq!(done["result"]["dialogs"], json!([]), "nothing opened after the answer: {done}");
        let h = stored_health(&e);
        assert_eq!(h["dialogs"], json!([{"type": "confirm", "message": "Continue?", "answered": "accepted"}]), "{h}");
        assert!(h["errors"].as_array().unwrap().iter().any(|x| x["text"] == "before the answer"), "other queued events are kept: {h}");
    }

    /// #2019 review: when a question stops the page's facts after the answer, the events already
    /// read are kept with it.
    #[test]
    fn events_read_before_a_question_stops_the_facts_are_kept() {
        let (client, ep) = stand_in_page(|s| {
            send_frame(s, json!({"method": cdp::DIALOG_OPENING, "params": {"type": "confirm", "message": "Continue?"}}));
            let first = read_frame(s).unwrap();
            send_frame(
                s,
                json!({"method": "Runtime.consoleAPICalled", "params": {"type": "error", "args": [{"type": "string", "value": "after the answer"}]}}),
            );
            send_frame(s, json!({"id": first["id"], "result": {}}));
            // Reading the page's facts is interrupted by the next question.
            reply_until(s, |m| m["method"] == "Runtime.evaluate").unwrap();
            send_frame(s, json!({"method": cdp::DIALOG_OPENING, "params": {"type": "prompt", "message": "Your team?", "defaultPrompt": ""}}));
            reply_until(s, |_| false);
        });
        let e = engine_held(client, &ep);
        let asked = answer(&e, json!({"accept": true, "settle_ms": 100})).unwrap();
        assert_eq!(asked["result"]["dialog_open"]["type"], "prompt", "{asked}");
        let h = stored_health(&e);
        assert!(h["errors"].as_array().unwrap().iter().any(|x| x["text"] == "after the answer"), "{h}");
        assert_eq!(h["dialogs"][1], json!({"type": "prompt", "message": "Your team?", "answered": "waiting"}), "{h}");
    }

    /// #2019 review: what was queued before an answer that runs out of time stays with the held
    /// question, so the try that reads the late outcome still reports it.
    #[test]
    fn events_queued_before_an_unconfirmed_answer_reach_the_retry() {
        let (client, ep) = stand_in_page(|s| {
            send_frame(s, json!({"method": cdp::DIALOG_OPENING, "params": {"type": "confirm", "message": "Continue?"}}));
            send_frame(
                s,
                json!({"method": "Runtime.consoleAPICalled", "params": {"type": "error", "args": [{"type": "string", "value": "before the answer"}]}}),
            );
            let first = read_frame(s).unwrap();
            std::thread::sleep(Duration::from_millis(1500));
            send_frame(s, json!({"id": first["id"], "result": {}}));
            reply_until(s, |_| false);
        });
        let e = engine_held_as(client, &ep, false);
        assert_eq!(answer(&e, json!({"accept": true, "timeout_secs": 1})).unwrap_err().code, ErrorCode::TimedOut);
        let read = answer(&e, json!({"accept": true, "settle_ms": 100})).unwrap();
        assert_eq!(read["result"]["answered"]["earlier"], true, "{read}");
        let h = stored_health(&e);
        assert!(h["errors"].as_array().unwrap().iter().any(|x| x["text"] == "before the answer"), "{h}");
        assert_eq!(h["dialogs"], json!([{"type": "confirm", "message": "Continue?", "answered": "accepted"}]), "{h}");
    }

    /// A stand-in page that asks `questions` in turn: each answer is replied to, then
    /// `after_answer[i]` is sent at once and `later[i]` (delay in ms, event) on a timer. The page's
    /// facts say "Done" once the last timed event of that answer was sent, "Loading" before.
    fn loading_page(questions: Vec<Value>, before: Vec<Value>, after_answer: Vec<Vec<Value>>, later: Vec<Vec<(u64, Value)>>) -> (CdpClient, Endpoint) {
        use std::sync::atomic::{AtomicBool, Ordering};
        stand_in_page(move |s| {
            let w = Arc::new(Mutex::new(s.try_clone().unwrap()));
            let say = |v: Value| {
                if let Ok(mut out) = w.lock() {
                    send_frame(&mut out, v);
                }
            };
            say(json!({"method": cdp::DIALOG_OPENING, "params": questions[0]}));
            before.into_iter().for_each(&say);
            let loaded = Arc::new(AtomicBool::new(true));
            let mut turn = 0;
            while let Some(m) = read_frame(s) {
                let result = match m["method"].as_str() {
                    Some("Runtime.evaluate") => {
                        let title = if loaded.load(Ordering::SeqCst) { "Done" } else { "Loading" };
                        json!({"result": {"type": "object", "value": {"url": "http://page.test/next", "title": title}}})
                    }
                    _ => json!({}),
                };
                say(json!({"id": m["id"], "result": result}));
                if m["method"] != "Page.handleJavaScriptDialog" {
                    continue;
                }
                after_answer.get(turn).cloned().unwrap_or_default().into_iter().for_each(&say);
                let timed = later.get(turn).cloned().unwrap_or_default();
                if !timed.is_empty() {
                    loaded.store(false, Ordering::SeqCst);
                    let (w2, loaded2) = (w.clone(), loaded.clone());
                    std::thread::spawn(move || {
                        let last = timed.len().saturating_sub(1);
                        for (i, (ms, v)) in timed.into_iter().enumerate() {
                            std::thread::sleep(Duration::from_millis(ms));
                            // Done before the last event goes out, so facts asked for once it
                            // arrives can't find the flag still unset.
                            if i == last {
                                loaded2.store(true, Ordering::SeqCst);
                            }
                            if let Ok(mut out) = w2.lock() {
                                send_frame(&mut out, v);
                            }
                        }
                    });
                }
                turn += 1;
                if let Some(q) = questions.get(turn) {
                    say(json!({"method": cdp::DIALOG_OPENING, "params": q}));
                }
            }
        })
    }

    fn frame(method: &str, id: &str) -> Value {
        json!({"method": method, "params": {"frameId": id}})
    }

    /// #2019 review: a load that started before the answer is waited for until each of its frames
    /// stops, so the page's facts aren't read mid-load: a child frame stopping first doesn't end
    /// the main frame's load, and the root's load event doesn't end a child's.
    #[test]
    fn a_load_under_way_before_the_answer_is_waited_for() {
        let confirm = json!({"type": "confirm", "message": "Leave for the next step?"});
        let started = vec![frame("Page.frameStartedLoading", "F"), frame("Page.frameStartedLoading", "G")];
        let orders = [
            vec![(300, frame("Page.frameStoppedLoading", "G")), (500, frame("Page.frameStoppedLoading", "F"))],
            vec![
                (300, frame("Page.frameStoppedLoading", "F")),
                (0, json!({"method": "Page.loadEventFired", "params": {}})),
                (500, frame("Page.frameStoppedLoading", "G")),
            ],
        ];
        for order in orders {
            let (client, ep) = loading_page(vec![confirm.clone()], started.clone(), vec![], vec![order.clone()]);
            let e = engine_held_as(client, &ep, false);
            let done = answer(&e, json!({"accept": true, "settle_ms": 100})).unwrap();
            assert_eq!(done["result"]["title"], "Done", "read after the last frame stopped ({order:?}): {done}");
        }
    }

    /// #2019 review: a frame that started loading after one answer, and was still loading when
    /// the page asked again, is still waited for after the next answer.
    #[test]
    fn a_load_is_waited_for_across_questions() {
        let questions = vec![json!({"type": "confirm", "message": "Continue?"}), json!({"type": "confirm", "message": "Are you sure?"})];
        let (client, ep) = loading_page(
            questions,
            vec![],
            vec![vec![frame("Page.frameStartedLoading", "F")]],
            vec![vec![], vec![(800, frame("Page.frameStoppedLoading", "F"))]],
        );
        let e = engine_held(client, &ep);
        let asked = answer(&e, json!({"accept": true, "settle_ms": 100})).unwrap();
        assert_eq!(asked["result"]["dialog_open"]["message"], "Are you sure?", "{asked}");
        let done = answer(&e, json!({"accept": true, "settle_ms": 100})).unwrap();
        assert_eq!(done["result"]["title"], "Done", "F's load from before the second question was waited for: {done}");
    }

    /// #2019 review: once the answer is given, failing to read the page after it is said with the
    /// answer, not returned as the tool failing (a retry would find nothing left to answer).
    #[test]
    fn an_answer_is_reported_even_when_the_page_cant_be_read_after() {
        let (client, ep) = stand_in_page(|s| {
            send_frame(s, json!({"method": cdp::DIALOG_OPENING, "params": {"type": "confirm", "message": "Continue?"}}));
            let first = read_frame(s).unwrap();
            send_frame(s, json!({"id": first["id"], "result": {}}));
            // The browser goes away as the page is about to be read.
            reply_until(s, |m| m["method"] == "Runtime.evaluate");
            let _ = s.shutdown(std::net::Shutdown::Both);
        });
        let e = engine_held(client, &ep);
        let done = answer(&e, json!({"accept": true, "settle_ms": 100})).unwrap();
        assert_eq!(done["result"]["answered"]["accept"], true, "{done}");
        assert_eq!(done["coverage"]["status"], "partial", "{done}");
        assert!(done["limitations"].as_array().unwrap().iter().any(|l| l["code"] == "page_unread" || l["id"] == "page_unread"), "{}", done["limitations"]);
        assert!(done["summary"].as_str().unwrap().contains("wasn't read afterwards"), "{}", done["summary"]);
        assert_eq!(stored_health(&e)["dialogs"][0]["answered"], "accepted");
    }

    /// #2019 review: a question that stops a navigation is reported with the page that asked it,
    /// not as the requested page opened.
    #[test]
    fn a_question_before_the_navigation_names_the_page_that_asked() {
        let none = Value::Null;
        assert_eq!(page_at(&none, Some("https://old.test/a".into()), "https://new.test/"), "https://old.test/a");
        assert_eq!(page_at(&json!({"url": "https://new.test/x"}), Some("https://old.test/a".into()), "https://new.test/"), "https://new.test/x");
        assert_eq!(page_at(&none, None, "https://new.test/"), "https://new.test/");
        let s = opened_said("https://new.test/", "https://old.test/a", "", "", true);
        assert_eq!(s, "Went to https://new.test/; the page at https://old.test/a asked a question before it could be read.");
        assert_eq!(opened_said("https://new.test/", "https://new.test/", "", "", true), "Opened https://new.test/.", "the new page asked");
        assert_eq!(opened_said("https://new.test/", "https://new.test/", "", "Home", false), "Opened https://new.test/: \"Home\".");
    }

    /// #2019 review: a question the page asks while web_site_open reads its facts is reported as
    /// the question, with the page's earlier health kept; a read failure without one is returned.
    #[test]
    fn a_question_while_the_facts_are_read_is_reported() {
        let (mut client, _) = stand_in_page(|s| {
            let read = read_frame(s).unwrap();
            assert_eq!(read["method"], "Runtime.evaluate");
            send_frame(s, json!({"method": "Runtime.consoleAPICalled", "params": {"type": "error", "args": [{"value": "boom"}]}}));
            send_frame(s, json!({"method": cdp::DIALOG_OPENING, "params": {"type": "confirm", "message": "Stay?", "url": "http://page.test/"}}));
            let _ = reply_until(s, |_| false);
        });
        let dl = Deadline::new(Duration::from_secs(5), crate::tool::CancelToken::default());
        let before = json!({"errors": [{"kind": "script", "text": "earlier", "url": ""}], "failed_requests": []});
        let (facts, health) = facts_unless_asked(&mut client, before, &dl).unwrap();
        assert!(facts.is_none());
        assert_eq!(health["dialog_open"]["message"], "Stay?", "{health}");
        assert_eq!(health["dialogs"][0]["answered"], "waiting", "{health}");
        let texts = health["errors"].to_string();
        assert!(texts.contains("earlier") && texts.contains("boom"), "{health}");
        let (mut lost, _) = stand_in_page(|s| {
            let _ = read_frame(s);
            let _ = s.shutdown(std::net::Shutdown::Both);
        });
        assert!(facts_unless_asked(&mut lost, json!({"errors": [], "failed_requests": []}), &dl).is_err());
    }

    /// #2019 review: a retry whose liveness probe fails still answers on the held connection,
    /// which is the only one that can read an earlier answer.
    #[test]
    fn a_held_question_is_answered_without_the_liveness_probe() {
        let (client, _) = stand_in_page(|s| {
            send_frame(s, json!({"method": cdp::DIALOG_OPENING, "params": {"type": "confirm", "message": "Continue?"}}));
            let first = read_frame(s).unwrap();
            send_frame(s, json!({"id": first["id"], "result": {}}));
            let _ = reply_until(s, |_| false);
        });
        // The site's endpoint hangs up on every request, so /json/version fails; the page
        // connection is fine. The listener stays bound for the whole test.
        let mute = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = mute.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for c in mute.incoming().flatten() {
                drop(c);
            }
        });
        let gone = cdp::parse_endpoint(&format!("http://127.0.0.1:{port}")).unwrap();
        let e = engine_held(client, &gone);
        let done = answer(&e, json!({"accept": true, "settle_ms": 100})).unwrap();
        assert_eq!(done["result"]["answered"]["accept"], true, "{done}");
        assert_eq!(done["result"]["title"], "Done", "{done}");
    }

    /// #2019 review: the typed text isn't kept as a parameter, and what the page itself shows
    /// or logs afterwards is reported as the page's (the narrow guarantee the tool states).
    #[test]
    fn typed_text_the_page_repeats_is_reported_as_the_pages() {
        let (client, ep) = stand_in_page(|s| {
            send_frame(s, json!({"method": cdp::DIALOG_OPENING, "params": {"type": "prompt", "message": "Your team?", "defaultPrompt": ""}}));
            let first = read_frame(s).unwrap();
            send_frame(s, json!({"id": first["id"], "result": {}}));
            send_frame(s, json!({"method": "Runtime.consoleAPICalled", "params": {"type": "error", "args": [{"value": "unknown team Design-7"}]}}));
            let read = reply_until(s, |m| m["method"] == "Runtime.evaluate").unwrap();
            send_frame(
                s,
                json!({"id": read["id"], "result": {"result": {"type": "object", "value": {"url": "http://page.test/t", "title": "Team Design-7"}}}}),
            );
            let _ = reply_until(s, |_| false);
        });
        let e = engine_held(client, &ep);
        let done = answer(&e, json!({"accept": true, "prompt_text": "Design-7", "settle_ms": 100})).unwrap();
        assert_eq!(done["params"], json!({"accept": true, "prompt_text_given": true}), "the parameter isn't kept");
        assert_eq!(done["result"]["title"], "Team Design-7", "the page's own title is reported: {done}");
        assert!(done["result"]["health"]["errors"].to_string().contains("Design-7"), "the page's own console error is reported: {done}");
    }

    /// #2019 review: a console error the page reports while its facts are read is in the answer's
    /// health and the stored health, not dropped because the read succeeded.
    #[test]
    fn an_error_while_the_page_is_read_is_in_its_health() {
        let (client, ep) = stand_in_page(|s| {
            send_frame(s, json!({"method": cdp::DIALOG_OPENING, "params": {"type": "confirm", "message": "Continue?"}}));
            let first = read_frame(s).unwrap();
            send_frame(s, json!({"id": first["id"], "result": {}}));
            let read = reply_until(s, |m| m["method"] == "Runtime.evaluate").unwrap();
            send_frame(s, json!({"method": "Runtime.consoleAPICalled", "params": {"type": "error", "args": [{"value": "late failure"}]}}));
            send_frame(s, json!({"id": read["id"], "result": {"result": {"type": "object", "value": {"url": "http://page.test/done", "title": "Done"}}}}));
            let _ = reply_until(s, |_| false);
        });
        let e = engine_held(client, &ep);
        let done = answer(&e, json!({"accept": true, "settle_ms": 100})).unwrap();
        assert_eq!(done["result"]["title"], "Done", "{done}");
        let has = |h: &Value| h["errors"].as_array().is_some_and(|es| es.iter().any(|x| x["text"] == "late failure"));
        assert!(has(&done["result"]["health"]), "in the answer's health: {done}");
        assert!(has(&stored_health(&e)), "in the stored health: {}", stored_health(&e));
    }

    /// #2019 review: when the page's own script fails as it's read after an answer, its exception
    /// text (which could repeat what was typed into a prompt) is kept out of the evidence.
    #[test]
    fn a_page_script_failure_after_an_answer_keeps_its_text_out() {
        let (client, ep) = stand_in_page(|s| {
            send_frame(s, json!({"method": cdp::DIALOG_OPENING, "params": {"type": "prompt", "message": "Your code?", "defaultPrompt": ""}}));
            let first = read_frame(s).unwrap();
            send_frame(s, json!({"id": first["id"], "result": {}}));
            let read = reply_until(s, |m| m["method"] == "Runtime.evaluate").unwrap();
            let thrown = json!({"exceptionDetails": {"text": "Uncaught", "exception": {"description": "Error: bad code SECRET-4321"}}});
            send_frame(s, json!({"id": read["id"], "result": thrown}));
            let _ = reply_until(s, |_| false);
        });
        let e = engine_held(client, &ep);
        let done = answer(&e, json!({"accept": true, "prompt_text": "SECRET-4321", "settle_ms": 100})).unwrap();
        assert_eq!(done["result"]["answered"]["accept"], true, "{done}");
        assert_eq!(done["coverage"]["status"], "partial", "{done}");
        assert!(!done.to_string().contains("SECRET-4321"), "the page's text was kept: {done}");
        assert!(done["summary"].as_str().unwrap().contains("the page's script failed"), "{}", done["summary"]);
    }

    /// A timeout keeps its own reason: only the page's script text is replaced.
    #[test]
    fn only_a_page_script_failure_is_marked() {
        let thrown = ToolError::new(ErrorCode::ProgramFailed, "reading the page failed: x").with_details(json!({PAGE_SCRIPT_FAILED: true}));
        assert!(page_script_failed(&thrown));
        assert!(!page_script_failed(&ToolError::new(ErrorCode::TimedOut, "out of time")));
    }

    /// #2019 review: a wait that reaches its bound with a frame still loading says so; one whose
    /// frames stopped doesn't.
    #[test]
    fn a_wait_that_ends_mid_load_says_so() {
        let (mut client, _) = stand_in_page(|s| {
            send_frame(s, json!({"method": "Page.frameStartedLoading", "params": {"frameId": "F"}}));
        });
        let dl = Deadline::new(Duration::from_secs(10), crate::tool::CancelToken::default());
        let mut seen = Vec::new();
        client.pump_until(&Deadline::new(Duration::from_millis(300), crate::tool::CancelToken::default())).unwrap();
        let started = std::time::Instant::now();
        assert!(wait_after_answer_within(&mut client, 0, Duration::from_millis(700), &mut seen, &dl).unwrap(), "F never stopped");
        assert!(started.elapsed() >= Duration::from_millis(600), "waited to its bound: {:?}", started.elapsed());
        let (mut quiet, _) = stand_in_page(|s| {
            send_frame(s, json!({"method": "Page.frameStartedLoading", "params": {"frameId": "F"}}));
            send_frame(s, json!({"method": "Page.frameStoppedLoading", "params": {"frameId": "F"}}));
        });
        quiet.pump_until(&Deadline::new(Duration::from_millis(300), crate::tool::CancelToken::default())).unwrap();
        assert!(!wait_after_answer_within(&mut quiet, 0, Duration::from_millis(700), &mut seen, &dl).unwrap(), "F stopped");
    }

    /// #2019 review: the previous page's load event, arriving while the navigation is sent, doesn't
    /// end the wait for the new document; only a load after that document committed does.
    #[test]
    fn the_previous_pages_load_doesnt_end_the_wait() {
        let (mut client, _) = stand_in_page(|s| {
            send_frame(s, json!({"method": "Page.loadEventFired", "params": {}}));
            std::thread::sleep(Duration::from_millis(800));
            send_frame(s, json!({"method": "Page.frameNavigated", "params": {"frame": {"id": "M", "loaderId": "OTHER"}}}));
            send_frame(s, json!({"method": "Page.frameNavigated", "params": {"frame": {"id": "C", "parentId": "M", "loaderId": "L2"}}}));
            send_frame(s, json!({"method": "Page.loadEventFired", "params": {}}));
            std::thread::sleep(Duration::from_millis(400));
            send_frame(s, json!({"method": "Page.frameNavigated", "params": {"frame": {"id": "M", "loaderId": "L2"}}}));
            send_frame(s, json!({"method": "Page.loadEventFired", "params": {}}));
        });
        let dl = Deadline::new(Duration::from_secs(10), crate::tool::CancelToken::default());
        client.pump_until(&Deadline::new(Duration::from_millis(200), crate::tool::CancelToken::default())).unwrap();
        let started = std::time::Instant::now();
        wait_for_page(&mut client, 0, Some("L2"), &mut Vec::new(), &dl).unwrap();
        let took = started.elapsed();
        assert!(took >= Duration::from_millis(800) && took < Duration::from_secs(5), "ended at the new document's load: {took:?}");
        // Without a loader (a same-document navigation), the first load event ends it.
        let (mut same, _) = stand_in_page(|s| send_frame(s, json!({"method": "Page.loadEventFired", "params": {}})));
        same.pump_until(&Deadline::new(Duration::from_millis(200), crate::tool::CancelToken::default())).unwrap();
        let started = std::time::Instant::now();
        wait_for_page(&mut same, 0, None, &mut Vec::new(), &dl).unwrap();
        assert!(started.elapsed() < Duration::from_millis(500), "{:?}", started.elapsed());
    }

    /// #2019 review: an answer that runs out of time keeps the question for another try, and the
    /// try reads what that answer did instead of sending one that would land on the next question.
    #[test]
    fn an_answer_out_of_time_is_kept_and_its_late_outcome_used() {
        let (client, ep) = stand_in_page(|s| {
            send_frame(s, json!({"method": cdp::DIALOG_OPENING, "params": {"type": "confirm", "message": "Continue?"}}));
            let first = read_frame(s).unwrap();
            assert_eq!(first["params"], json!({"accept": true}));
            // Taken after its call ran out of time, and the page asks next.
            std::thread::sleep(Duration::from_millis(1500));
            send_frame(s, json!({"id": first["id"], "result": {}}));
            send_frame(s, json!({"method": cdp::DIALOG_OPENING, "params": {"type": "prompt", "message": "Your team?", "defaultPrompt": ""}}));
            // Nothing more is answered: the retry sent no answer of its own.
            let next = reply_until(s, |m| m["method"] == "Page.handleJavaScriptDialog");
            assert!(next.is_none(), "a second answer reached the page: {next:?}");
        });
        let e = engine_held(client, &ep);
        let late = answer(&e, json!({"accept": true, "timeout_secs": 1})).unwrap_err();
        assert_eq!(late.code, ErrorCode::TimedOut, "{late:?}");
        assert!(late.remediation.as_deref().is_some_and(|r| r.contains(DIALOG_ANSWER_TOOL)), "{late:?}");
        let retry = answer(&e, json!({"accept": false, "settle_ms": 100})).unwrap();
        assert_eq!(retry["result"]["answered"]["accept"], true, "the answer the page took: {retry}");
        assert_eq!(retry["result"]["answered"]["earlier"], true);
        assert_eq!(retry["result"]["dialog_open"]["type"], "prompt", "{retry}");
        assert!(retry["summary"].as_str().unwrap().contains("this one wasn't sent"), "{}", retry["summary"]);
        let h = stored_health(&e);
        assert_eq!(h["dialogs"][0]["answered"], "accepted", "{h}");
        // Leaving the page dismisses the held prompt and closes the connection.
        drop(e);
    }

    /// A question asked while the page settles ends the wait then, not at the end of the settle time.
    #[test]
    fn a_question_while_the_page_settles_ends_the_wait() {
        let (mut client, _) = stand_in_page(|s| {
            send_frame(s, json!({"method": "Page.loadEventFired", "params": {}}));
            std::thread::sleep(Duration::from_millis(300));
            send_frame(s, json!({"method": cdp::DIALOG_OPENING, "params": {"type": "confirm", "message": "Leave a review?"}}));
        });
        let dl = Deadline::new(Duration::from_secs(30), crate::tool::CancelToken::default());
        let mut seen = Vec::new();
        let started = std::time::Instant::now();
        wait_for_page(&mut client, 20_000, None, &mut seen, &dl).unwrap();
        assert!(started.elapsed() < Duration::from_secs(5), "ended at the question: {:?}", started.elapsed());
        assert_eq!(client.waiting_dialog().map(|d| d["type"].clone()), Some(json!("confirm")));
        assert!(seen.iter().any(|e| e.method == cdp::DIALOG_OPENING), "the question is among the events read");
    }

    #[test]
    fn readme_shows_breakpoints_and_the_layout_at_each_width() {
        let b = json!({"url": "https://ex.com/", "breakpoints": [{"query": "min-width 768px", "px": 768, "count": 3}],
        "design": {"spacing": [{"value": "96px", "count": 20}], "max_widths": [{"value": "1280px", "count": 4}]},
        "assets": {"fonts": ["https://ex.com/f.woff2"]},
        "layouts": [
            {"width": 1440, "page_w": 1440, "type": {"h1": 64, "h2": 40, "h3": 24, "body": 16},
             "blocks": [{"tag": "section", "heading": "Plans", "pad": [96, 32, 96, 32], "gap": 24, "per_row": 3, "column": {"max_w": 1280, "x": 80}, "background": "#12181f"}]},
            {"width": 390, "page_w": 512, "type": {"h1": 40}, "blocks": [{"tag": "header", "pad": [16, 16, 16, 16], "per_row": 1}]}
        ]});
        let md = readme(&b, &[], (0, 0), false);
        assert!(md.contains("switches layout at: min-width 768px") && md.contains("`96px`") && md.contains("`1280px`") && md.contains("Font files: 1"), "{md}");
        assert!(md.contains("### 1440 px (no sideways scroll)") && md.contains("Headings 64/40/24 px, text 16 px"), "{md}");
        assert!(md.contains("- section \"Plans\": padding 96 32 96 32, gap 24, 3 per row, column 1280 px at x 80, background #12181f"), "{md}");
        assert!(md.contains("### 390 px (page 512 px wide, so it scrolls sideways)") && md.contains("- header: padding 16 16 16 16\n"), "{md}");
        let cross = readme(&json!({"unreadable_stylesheets": 2}), &[], (0, 0), false);
        assert!(cross.contains("2 stylesheet(s) from other sites"), "{cross}");
    }

    #[test]
    fn a_visitors_view_keeps_what_shows_and_drops_the_pages_code() {
        let mut b = json!({
            "title": "Plans", "meta": {"description": "hidden"}, "breakpoints": [{"query": "min-width 768px"}], "unreadable_stylesheets": 1,
            "health": {"errors": [{"text": "boom"}]}, "design": {"fonts": [{"value": "Inter", "count": 9}], "css_variables": {"--brand": "#123"}, "font_faces": [{"family": "Inter"}]},
            "assets": {"images": [{"src": "https://cdn.test/a.png", "w": 40, "h": 30, "alt": "logo"}], "scripts": ["https://cdn.test/app.js"], "requests": [{"url": "https://api.test/x"}]},
            "headings": [{"level": 1, "text": "Plans"}], "text": "Plans Pricing", "layouts": [{"width": 1440, "page_w": 1440}],
            "outline": [{"tag": "img", "class": "w-full rounded-xl", "src": "https://cdn.test/hero.webp?dpl=dpl_abc123#x"}, {"tag": "a", "class": "btn", "href": "https://shop.test/plans?ref=sk-test-123"}],
            "links": [{"text": "Plans", "href": "https://shop.test/plans?session=abc"}],
            "a11y": {"unnamed_examples": ["button.icon-btn", "a"]}
        });
        visitor_view(&mut b);
        // #156: the page's class names and the query strings on its addresses aren't a visitor's view.
        assert!(b["outline"].as_array().unwrap().iter().all(|i| i.get("class").is_none()));
        assert_eq!(b["outline"][0]["src"], "https://cdn.test/hero.webp");
        assert_eq!(b["outline"][1]["href"], "https://shop.test/plans");
        assert_eq!(b["links"][0]["href"], "https://shop.test/plans");
        assert_eq!(b["a11y"]["unnamed_examples"], json!(["button", "a"]));
        assert!(!b.to_string().contains("dpl_") && !b.to_string().contains("sk-test"));
        for gone in [
            "/meta",
            "/breakpoints",
            "/unreadable_stylesheets",
            "/health",
            "/design/css_variables",
            "/design/font_faces",
            "/assets/scripts",
            "/assets/requests",
            "/assets/images/0/src",
        ] {
            assert!(b.pointer(gone).is_none(), "{gone} isn't something a visitor sees");
        }
        assert_eq!((b["assets"]["images"][0]["w"].as_u64(), b["text"].as_str()), (Some(40), Some("Plans Pricing")));
        let md = readme(&b, &[], (0, 0), true);
        assert!(
            md.contains("Clean room: only what a visitor sees") && !md.contains("Breakpoints") && !md.contains("Page health") && !md.contains("page.html"),
            "{md}"
        );
        assert!(md.contains("### 1440 px"), "{md}");
    }

    #[test]
    fn capture_without_open_site_is_a_clear_error() {
        let e = crate::Engine::new(crate::testing::session());
        let r = e.call(
            "web_site_capture",
            json!({"output_directory": std::env::temp_dir().join("ca-never-created").display().to_string()}),
            crate::CallOptions::new(crate::Caller::Cli),
        );
        let err = r.err().map(|e| e.message).unwrap_or_default();
        assert!(err.contains("no website is open") || err.contains("Chromium") || err.contains("chromium"), "{err}");
    }

    /// #153: from the CLI, a site opened by an earlier command is gone, and the error says why and
    /// where the steps run together. A stand-in browser path gets past the engine check on any
    /// machine; nothing is started.
    #[test]
    fn a_cli_capture_without_a_site_says_each_command_starts_fresh() {
        let fix = |caller| {
            let mut s = crate::testing::session();
            s.settings.engines.tools.chromium = Some(std::env::current_exe().unwrap());
            let dir = tempfile::tempdir().unwrap();
            let out = dir.path().join("capture");
            let err = crate::Engine::new(s)
                .call("web_site_capture", json!({"output_directory": out.display().to_string()}), crate::CallOptions::new(caller))
                .unwrap_err();
            assert!(err.message.contains("no website is open"), "{err:?}");
            assert!(!out.exists());
            err.remediation.unwrap_or_default()
        };
        assert!(fix(crate::Caller::Cli).contains("Each CLI command starts its own browser") && fix(crate::Caller::Cli).contains("craft-anything-cli mcp"));
        assert_eq!(fix(crate::Caller::Mcp), "Open one first with web_site_open.");
    }

    /// A browser that exits before becoming ready gets the exit diagnostic, not the deadline
    /// diagnostic. The stand-in exits without opening a browser or a window.
    #[test]
    fn a_browser_exit_before_readiness_is_reported_as_an_exit() {
        let stand_in = if cfg!(windows) {
            std::path::PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into())).join("System32").join("where.exe")
        } else {
            std::path::PathBuf::from("/bin/false")
        };
        if !stand_in.is_file() {
            return;
        }
        let mut s = crate::testing::session();
        s.settings.engines.tools.chromium = Some(stand_in);
        let e = crate::Engine::new(s);
        let opts = crate::CallOptions::new(crate::Caller::Cli).with_consent(crate::Consent::Granted);
        // The error identifies which path won. A second wall-clock limit on the whole
        // call also times the test host scheduling, process startup and profile cleanup.
        let err = e.call("web_site_open", json!({"url": "https://example.com", "timeout_secs": 60}), opts).unwrap_err();
        assert_eq!(err.code, ErrorCode::ProgramFailed, "{err:?}");
        assert!(err.message.starts_with("Couldn't start the private browser") && err.message.contains("closed after"), "{}", err.message);
        assert!(err.remediation.is_some_and(|r| r.contains("Settings")));
    }

    /// #160: a section that only shows once it's scrolled into view is in the full-page picture.
    /// Live: it needs a Chromium-based browser, so it runs with `--ignored`.
    #[test]
    #[ignore]
    fn sections_shown_on_scroll_are_photographed() {
        use std::io::{Read, Write};
        let page = r#"<!doctype html><html><body style="margin:0">
<div style="height:1400px;background:#fff">Top</div>
<div id="late" style="height:600px;background:#fff">Shown on scroll</div>
<script>new IntersectionObserver(es => es.forEach(e => { if (e.isIntersecting) e.target.style.background = '#ff0000'; })).observe(document.getElementById('late'));</script>
</body></html>"#;
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for mut s in listener.incoming().flatten() {
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf);
                let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}", page.len());
            }
        });
        // In Clean room, a visitor's view: its evidence says so (#157).
        let mut s = crate::testing::live_session();
        s.settings.deep_mode = false;
        let e = crate::Engine::new(s);
        let cli = || crate::CallOptions::new(crate::Caller::Cli).with_consent(crate::Consent::Granted);
        e.call("web_site_open", json!({"url": url}), cli()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("capture");
        let v = e.call("web_site_capture", json!({"output_directory": out.display().to_string(), "widths": [1440]}), cli()).unwrap().to_json();
        assert_eq!(v["environment"]["clean_room"], true, "{}", v["environment"]);
        let shot = web_common::decode_png(&std::fs::read(out.join("screenshot-1440.png")).unwrap()).unwrap();
        let at = |x: u32, y: u32| shot.rgba.get(((y * shot.width + x) * 4) as usize..((y * shot.width + x) * 4 + 3) as usize).unwrap().to_vec();
        assert_eq!(at(20, 1700), [255, 0, 0], "the section shown on scroll is in the picture");
        assert_eq!(at(20, 200), [255, 255, 255]);
    }

    /// #158: after a site fails to open, a capture refuses instead of photographing the browser's
    /// error page as if it were the site. Live: it needs a Chromium-based browser (`--ignored`).
    #[test]
    #[ignore]
    fn the_browsers_error_page_is_not_captured() {
        let e = crate::Engine::new(crate::testing::live_session());
        let cli = || crate::CallOptions::new(crate::Caller::Cli).with_consent(crate::Consent::Granted);
        // Nothing listens on port 1, so the browser shows its own error page.
        let _ = e.call("web_site_open", json!({"url": "http://127.0.0.1:1/"}), cli());
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("capture");
        let err = e.call("web_site_capture", json!({"output_directory": out.display().to_string(), "widths": [1440]}), cli()).unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound, "{}", err.message);
        assert!(err.message.contains("didn't open"), "{}", err.message);
        assert!(!out.exists(), "no folder is made for a capture that can't happen");
    }

    /// #2018: a page that asks before it shows anything waits for the agent's answer, and a capture
    /// says why it can't start. Live: it needs a Chromium-based browser (`--ignored`).
    #[test]
    #[ignore]
    fn a_question_on_load_waits_for_the_agents_answer() {
        use std::io::{Read, Write};
        let page = r#"<!doctype html><html><head><title>Asking</title></head><body><script>
document.title = confirm('Continue to the new version?\nIt has a new layout.') ? 'Agreed' : 'Declined';
if (document.title === 'Agreed') { var t = prompt('Your team?', 'Sales'); document.title = t === null ? 'No team' : 'Team ' + t; }
</script></body></html>"#;
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for mut s in listener.incoming().flatten() {
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf);
                let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}", page.len());
            }
        });
        let e = crate::Engine::new(crate::testing::live_session());
        let cli = || crate::CallOptions::new(crate::Caller::Cli).with_consent(crate::Consent::Granted);
        let started = std::time::Instant::now();
        let opened = e.call("web_site_open", json!({"url": url}), cli()).unwrap().to_json();
        assert!(started.elapsed() < Duration::from_secs(30), "it says so at once instead of waiting out its deadline");
        assert_eq!(opened["result"]["dialog_open"], json!({"type": "confirm", "message": "Continue to the new version?"}), "{opened}");
        assert!(opened["summary"].as_str().unwrap().contains(DIALOG_ANSWER_TOOL), "{}", opened["summary"]);
        assert_eq!(opened["coverage"]["status"], "partial");

        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("capture");
        let err = e.call("web_site_capture", json!({"output_directory": out.display().to_string(), "widths": [1440]}), cli()).unwrap_err();
        assert!(err.message.contains("waiting for an answer to its confirm"), "{}", err.message);
        assert!(!out.exists(), "no folder for a capture that can't start");
        let asked = std::time::Instant::now();
        let shot = e.call("web_page_screenshot_take", json!({}), cli()).unwrap_err();
        assert!(shot.message.contains("waiting for an answer"), "the other browser tools say so too: {}", shot.message);
        assert!(asked.elapsed() < Duration::from_secs(10), "at once, not at their deadline");

        let agreed = e.call(DIALOG_ANSWER_TOOL, json!({"accept": true}), cli()).unwrap().to_json();
        assert_eq!(agreed["result"]["dialog_open"], json!({"type": "prompt", "message": "Your team?", "default_prompt": "Sales"}), "{agreed}");
        let typed = e.call(DIALOG_ANSWER_TOOL, json!({"accept": true, "prompt_text": "Design"}), cli()).unwrap().to_json();
        assert_eq!(typed["result"]["title"], "Team Design", "{typed}");
        assert_eq!(typed["result"]["dialog_open"], Value::Null);
        assert_eq!(typed["params"], json!({"accept": true, "prompt_text_given": true}), "what was typed isn't kept");
        let none = e.call(DIALOG_ANSWER_TOOL, json!({"accept": false}), cli()).unwrap_err();
        assert_eq!(none.code, ErrorCode::NotFound, "{}", none.message);
        let captured = e.call("web_site_capture", json!({"output_directory": out.display().to_string(), "widths": [1440]}), cli());
        assert!(captured.is_ok(), "once answered, the page can be captured: {:?}", captured.err());
    }

    /// An engine with `page` open in a real browser, served from a local port. Live: it needs a
    /// Chromium-based browser, so it looks for one the way the app does.
    fn serve_and_open(page: &'static str) -> crate::Engine {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for mut s in listener.incoming().flatten() {
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf);
                let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}", page.len());
            }
        });
        let e = crate::Engine::new(crate::testing::live_session());
        e.call("web_site_open", json!({"url": url}), crate::CallOptions::new(crate::Caller::Cli).with_consent(crate::Consent::Granted)).unwrap();
        e
    }

    /// #339: states captured at a phone width after a desktop width open the phone's own button,
    /// not the desktop one the earlier capture marked. Live (`--ignored`).
    #[test]
    #[ignore]
    fn states_at_a_second_width_open_that_widths_controls() {
        let e = serve_and_open(
            r#"<!doctype html><html><head><meta name="viewport" content="width=device-width"><style>
body{margin:0} .d{display:none} @media (min-width:800px){.d{display:block} .p{display:none}}
.panel{display:none;position:fixed;top:0;left:0;width:100%;height:200px}
</style></head><body style="padding-top:300px">
<button class="d" aria-expanded="false" data-panel="dp">Desktop options</button>
<button class="p" aria-expanded="false" data-panel="pp">Phone options</button>
<div id="dp" class="panel" style="background:#0000ff"></div><div id="pp" class="panel" style="background:#ff0000"></div>
<script>for (const b of document.querySelectorAll('button')) b.onclick = () => {
  const open = b.getAttribute('aria-expanded') !== 'true';
  b.setAttribute('aria-expanded', String(open));
  document.getElementById(b.dataset.panel).style.display = open ? 'block' : 'none';
};</script></body></html>"#,
        );
        let cli = || crate::CallOptions::new(crate::Caller::Cli).with_consent(crate::Consent::Granted);
        let dir = tempfile::tempdir().unwrap();
        let top_colour = |out: &Path| {
            let rows: Value = serde_json::from_str(&std::fs::read_to_string(out.join("states.json")).unwrap()).unwrap();
            let shot = web_common::decode_png(&std::fs::read(out.join("state-1.png")).unwrap()).unwrap();
            let at = shot.rgba.get(((10 * shot.width + 10) * 4) as usize..((10 * shot.width + 10) * 4 + 3) as usize).unwrap().to_vec();
            (rows["states"][0]["label"].as_str().unwrap().to_string(), at)
        };
        for (width, label, colour) in [(1440, "Desktop options", [0, 0, 255]), (390, "Phone options", [255, 0, 0])] {
            let out = dir.path().join(format!("states-{width}"));
            e.call("web_page_states_capture", json!({"output_directory": out.display().to_string(), "width": width, "max_states": 1}), cli()).unwrap();
            assert_eq!(top_colour(&out), (label.to_string(), colour.to_vec()), "at {width} px");
        }
        let _ = e.call("web_site_close", json!({}), cli());
    }

    /// #335: after photographing another tab, the tab that was selected is selected again, so the
    /// next capture finds the page as it was. Live (`--ignored`).
    #[test]
    #[ignore]
    fn the_tab_that_was_selected_is_selected_again() {
        let e = serve_and_open(
            r#"<!doctype html><html><body><div role="tablist">
<button role="tab" aria-selected="true">One</button><button role="tab" aria-selected="false">Two</button></div>
<script>const tabs = document.querySelectorAll('[role="tab"]');
for (const t of tabs) t.onclick = () => tabs.forEach(o => o.setAttribute('aria-selected', String(o === t)));</script></body></html>"#,
        );
        let cli = || crate::CallOptions::new(crate::Caller::Cli).with_consent(crate::Consent::Granted);
        let dir = tempfile::tempdir().unwrap();
        // Each capture opens the unselected tab: Two both times when One is selected again.
        for n in 1..=2 {
            let out = dir.path().join(format!("states-{n}"));
            e.call("web_page_states_capture", json!({"output_directory": out.display().to_string(), "max_states": 1}), cli()).unwrap();
            let rows: Value = serde_json::from_str(&std::fs::read_to_string(out.join("states.json")).unwrap()).unwrap();
            assert_eq!(rows["states"][0]["label"], "Two", "capture {n}: {rows}");
            assert_eq!(rows["states"][0]["changed"], json!(["tab selected"]), "capture {n}: {rows}");
        }
        let _ = e.call("web_site_close", json!({}), cli());
    }

    /// `capture.json` of `page`, served from a local port and captured at 1440 px. Live: it needs a
    /// Chromium-based browser.
    fn capture_of(page: &'static str) -> Value {
        let e = serve_and_open(page);
        let cli = || crate::CallOptions::new(crate::Caller::Cli).with_consent(crate::Consent::Granted);
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("capture");
        e.call("web_site_capture", json!({"output_directory": out.display().to_string(), "widths": [1440]}), cli()).unwrap();
        serde_json::from_str(&std::fs::read_to_string(out.join("capture.json")).unwrap()).unwrap()
    }

    /// #155: a pill made with `calc(infinity * 1px)` (Tailwind's `rounded-full`) is recorded as
    /// `9999px`, not the browser's clamped `3.35544e+07px`. Live (`--ignored`).
    #[test]
    #[ignore]
    fn an_endless_corner_radius_is_recorded_as_a_pill() {
        let page = r#"<!doctype html><html><body>
<a style="border-radius:calc(infinity * 1px);background:#1e66f5;padding:8px">One</a>
<a style="border-radius:calc(infinity * 1px) calc(infinity * 1px) 4px 4px;background:#1e66f5;padding:8px">Two</a>
<a style="border-radius:12px;background:#1e66f5;padding:8px">Three</a>
</body></html>"#;
        let capture = capture_of(page);
        let mut radii: Vec<&str> = capture["design"]["radii"].as_array().unwrap().iter().filter_map(|r| r["value"].as_str()).collect();
        radii.sort_unstable();
        assert_eq!(radii, ["12px", "9999px", "9999px 9999px 4px 4px"]);
    }

    /// #154: colours are the page's own CSS values, not a canvas read that can be off by one per
    /// channel; see-through ones keep their alpha. Live (`--ignored`).
    #[test]
    #[ignore]
    fn captured_colours_are_the_css_values() {
        let capture = capture_of(
            r#"<!doctype html><html><body style="background:#F5F2EC;color:#2D2B26"><p>Text</p><p style="background:rgba(30, 102, 245, 0.5)">Half</p></body></html>"#,
        );
        let values = |k: &str| capture["design"][k].as_array().unwrap().iter().filter_map(|c| c["value"].as_str().map(String::from)).collect::<Vec<_>>();
        assert!(values("backgrounds").contains(&"#f5f2ec".to_string()), "{:?}", values("backgrounds"));
        assert!(values("backgrounds").contains(&"#1e66f580".to_string()), "{:?}", values("backgrounds"));
        assert_eq!(values("text_colors"), ["#2d2b26"]);
    }
}
