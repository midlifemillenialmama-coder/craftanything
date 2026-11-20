//! Finding Ghidra and the Java it runs on, and getting both for the person once they agree:
//! pinned releases, each checked against its publisher's SHA-256 before anything is unpacked.

use craft_anything_evidence::{ErrorCode, ToolError};
use craft_anything_platform::process::Cancel;
use std::path::{Path, PathBuf};

use super::{Config, GhidraError};
use crate::java::{self, JavaVerdict};
use crate::version::{Version, parse as version_parse, version_order};

/// Ghidra's engine id, as the engine table, Settings and every listing name it.
pub const ID: &str = "ghidra";
/// The engine store folder the Java for Ghidra is unpacked into; the Java runtime engine is found
/// there too (the engine table's `Install::Guided::folder`).
pub const JAVA_FOLDER: &str = "jdk";
/// The Ghidra release Craft Anything installs (it needs Java 25).
pub const VERSION: &str = "12.2";
/// What the person downloads, for the question we ask first (Ghidra 543 MB + Java 120–140 MB,
/// rounded up to the next 10 MB).
pub const DOWNLOAD_MB: u32 = 700;
/// Ghidra 12.2 runs on Java 25; a Java below that is refused before Ghidra starts, not hours
/// into an analysis.
pub const JAVA_MAJOR: u32 = 25;
/// What a kept legacy install (12.1.4) needs at least, while it may still be used.
pub const LEGACY_JAVA_MAJOR: u32 = 21;
/// Whether a Ghidra older than [`VERSION`] may still be used: `AllowWithHint` keeps it usable
/// and says a newer one exists; `Refuse` stops it. One constant, one policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyPolicy {
    AllowWithHint,
    Refuse,
}
pub const LEGACY_POLICY: LegacyPolicy = LegacyPolicy::AllowWithHint;
/// Where to get Ghidra by hand when we can't install it here.
pub const PAGE: &str = "https://github.com/NationalSecurityAgency/ghidra/releases";

/// The version below which an install counts as legacy: [`VERSION`].
pub fn min_current() -> Version {
    // An unparsable VERSION would leave nothing below it; `pins_come_from_the_engine_table` guards the parse.
    Version(version_parse(VERSION).map(|v| v.0).unwrap_or_default())
}

/// Whether a version hint names a Ghidra older than [`VERSION`]. A name without a readable
/// version is never assumed old: it gets the current Java floor and the current verdict.
fn is_below_current(hint: Option<&Version>) -> bool {
    hint.is_some_and(|v| *v < min_current())
}

/// The Java floor a Ghidra of this version needs: 25 for this release and anything of unknown
/// version, 21 for a kept legacy install.
fn java_floor(hint: Option<&Version>) -> u32 {
    if is_below_current(hint) { LEGACY_JAVA_MAJOR } else { JAVA_MAJOR }
}

/// The host's pin for a guided engine (`ghidra`, `java`) from the engine table.
fn pin(id: &str) -> Option<&'static crate::Pin> {
    crate::engine(id).and_then(crate::ExternalEngine::host_pin)
}

/// The executables to look for on this computer.
fn host_exes() -> &'static [&'static str] {
    crate::engine(ID).map(crate::ExternalEngine::host_exes).unwrap_or(&[])
}

/// Whether [`install`] works on this computer: Ghidra and its Java both have a pin here (Java is
/// pinned for Windows x64 and for Linux and macOS on x64 and arm64).
pub fn can_install() -> bool {
    pin(ID).is_some() && pin("java").is_some()
}

/// Where a found Ghidra comes from, shown in listings and the doctor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhidraSource {
    /// The folder set in Settings → Engines.
    UserFolder,
    /// The managed install in the engine store.
    EngineStore,
    /// An `analyzeHeadless` on PATH.
    Path,
    /// A system package's folder (`/opt/ghidra` on Arch, `/usr/share/ghidra` on Debian).
    StandardLocation,
    /// A package manager's copy, from the engine table's package paths (`Homebrew`).
    PackageManager(&'static str),
    /// An unpacked copy in a usual place (Downloads, the home folder…).
    UnpackedFolder,
}

impl GhidraSource {
    /// Where this source says the install came from, for a listing's detail line.
    pub fn label(&self) -> String {
        match self {
            GhidraSource::UserFolder => "the folder set in Settings → Engines".into(),
            GhidraSource::EngineStore => "the engine store".into(),
            GhidraSource::Path => "PATH".into(),
            GhidraSource::StandardLocation => "a standard install folder".into(),
            GhidraSource::PackageManager(who) => (*who).to_string(),
            GhidraSource::UnpackedFolder => "an unpacked copy".into(),
        }
    }
}

/// A Ghidra found on this computer. `version_hint` is read from folder names alone: it decides
/// which Java floor applies and what the status shows; the version recorded as evidence is the
/// one Ghidra itself reports once it answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GhidraInstall {
    /// The headless launcher, ready to run.
    pub launcher: PathBuf,
    pub source: GhidraSource,
    pub version_hint: Option<Version>,
}

/// Whether Ghidra is usable, and when it isn't, why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhidraState {
    /// Found, with Java good enough for its version.
    Ready,
    /// A legacy install (below `12.2`) kept in use, with a hint that a newer one exists.
    Legacy,
    /// A legacy install refused (only under `LegacyPolicy::Refuse`).
    UnsupportedVersion,
    /// A folder for Ghidra is there but holds no headless launcher.
    Incomplete,
    /// Found, but the Java it will run on is known to be too old for it.
    JavaTooOld,
    /// Nothing found.
    NotFound,
}

impl GhidraState {
    /// Whether Deep mode may run Ghidra in this state.
    pub fn usable(&self) -> bool {
        matches!(self, GhidraState::Ready | GhidraState::Legacy)
    }
}

/// The one verdict on Ghidra that every listing shows: Settings → Engines, `engines_list`, the
/// doctor and `Requires::Ghidra` all take theirs from here, so none of them can disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GhidraStatus {
    pub state: GhidraState,
    pub install: Option<GhidraInstall>,
    pub java: GhidraJava,
}

/// The version a launcher's folder name carries: the launcher sits in `support/`, so the
/// install's top folder is above it — its own name for a release (`ghidra_12.2_PUBLIC`), or one
/// level higher only for a package manager's `libexec/support/…` layout
/// (`Cellar/ghidra/12.2/libexec/support/analyzeHeadless`, which is where Homebrew's `opt/ghidra`
/// link leads). Links are resolved first, so a link's own name never stands in for the version,
/// and any other parent's name (`user1`, say) is never read as one.
fn version_hint_of(launcher: &Path) -> Option<Version> {
    let launcher = std::fs::canonicalize(launcher).unwrap_or_else(|_| launcher.to_path_buf());
    let root = launcher.parent()?.parent()?;
    if let Some(v) = version_parse(&root.file_name()?.to_string_lossy()) {
        return Some(v);
    }
    if root.file_name()? != "libexec" {
        return None;
    }
    version_parse(&root.parent()?.file_name()?.to_string_lossy())
}

fn install_of(launcher: PathBuf, source: GhidraSource) -> GhidraInstall {
    let version_hint = version_hint_of(&launcher);
    GhidraInstall { launcher, source, version_hint }
}

/// The one place Ghidra is looked for, in a fixed order — the folder the person picked, the
/// engine store (newest version first), PATH, standard install folders, a package manager's
/// copy, then the usual unpacked places — stopping at the first hit. Within one source the
/// newest version wins, by the numbers (`12.10.0` beats `12.2`, which text order would get
/// backwards). Read-only: nothing is run, nothing is written.
pub fn locate_install(install_dir: Option<&Path>, store: &Path) -> Option<GhidraInstall> {
    locate_install_limited(install_dir, store, true)
}

/// [`locate_install`], but `look_around = false` keeps to what the settings and the managed
/// store say: a test session doesn't look around the computer.
pub fn locate_install_limited(install_dir: Option<&Path>, store: &Path, look_around: bool) -> Option<GhidraInstall> {
    if let Some(dir) = install_dir {
        let found = if dir.is_file() { launcher_from_file(dir) } else { in_root_or_child(dir, launcher) };
        if let Some(launcher) = found {
            return Some(install_of(launcher, GhidraSource::UserFolder));
        }
    }
    if let Some(launcher) = crate::in_managed(&store.join(ID), host_exes()) {
        return Some(install_of(launcher, GhidraSource::EngineStore));
    }
    if !look_around {
        return None;
    }
    if let Some(launcher) = crate::find_on_path(host_exes()) {
        return Some(install_of(launcher, GhidraSource::Path));
    }
    if let Some(launcher) = crate::well_known(ID).into_iter().find(|p| p.is_file()) {
        return Some(install_of(launcher, GhidraSource::StandardLocation));
    }
    if let Some((launcher, who)) = crate::engine(ID).and_then(crate::find_in_package_paths) {
        return Some(install_of(launcher, GhidraSource::PackageManager(who)));
    }
    let e = crate::engine(ID)?;
    crate::find_in_known_roots(ID, e.host_exes(), &e.known_root_folders()).map(|launcher| install_of(launcher, GhidraSource::UnpackedFolder))
}

/// Whether Ghidra is usable on this computer, and when it isn't, why: the one verdict that
/// `engines_list`, the Settings page, the doctor and `Requires::Ghidra` all show. `configured`
/// is the Java set for Ghidra (`Settings::ghidra_java`). Read-only, deterministic, and it never
/// starts a process — the Java version comes from the JDK's own `release` file.
pub fn status(install_dir: Option<&Path>, configured: Option<&Path>, store: &Path) -> GhidraStatus {
    status_limited(install_dir, configured, store, true)
}

/// [`status`], but `look_around = false` keeps to what the settings and the managed store say: a
/// test session doesn't look around the computer.
pub fn status_limited(install_dir: Option<&Path>, configured: Option<&Path>, store: &Path, look_around: bool) -> GhidraStatus {
    let inherited = std::env::var_os("JAVA_HOME").map(PathBuf::from);
    let install = locate_install_limited(install_dir, store, look_around);
    // The Java a launch would actually use: the one set for Ghidra, else the managed JDK a guided
    // install brought — the same fallback `Settings::ghidra_java()` hands to Ghidra. Judging the
    // inherited `JAVA_HOME` instead would call a usable install "Java too old".
    let effective = java_home(configured, store);
    let java = install.as_ref().map_or(GhidraJava::Missing, |i| java_in_use(&i.launcher, effective.as_deref(), inherited.as_deref()));
    let Some(install) = install else {
        // A folder that is there but holds no launcher is an incomplete install, not a missing
        // one: the person sees which folder was checked, not just "nothing".
        let incomplete = install_dir.is_some_and(Path::is_dir)
            || store.join(ID).is_dir()
            || look_around
                && crate::engine(ID).is_some_and(|e| e.package_paths.iter().any(|p| Path::new(p.prefix).join("opt").join(p.package).join("libexec").is_dir()));
        let state = if incomplete { GhidraState::Incomplete } else { GhidraState::NotFound };
        return GhidraStatus { state, install: None, java };
    };
    let below = is_below_current(install.version_hint.as_ref());
    if below && LEGACY_POLICY == LegacyPolicy::Refuse {
        return GhidraStatus { state: GhidraState::UnsupportedVersion, install: Some(install), java };
    }
    let state = match &java {
        GhidraJava::TooOld { .. } | GhidraJava::NotJava(_) => GhidraState::JavaTooOld,
        _ if below => GhidraState::Legacy,
        _ => GhidraState::Ready,
    };
    GhidraStatus { state, install: Some(install), java }
}

/// Ghidra's headless launcher: whatever [`locate_install`] finds first, in the folder the person
/// picked, the managed install, PATH, a standard install folder, a package manager's copy or the
/// folders where an unpacked copy is usually kept (#740).
pub fn headless(install_dir: Option<&Path>, store: &Path) -> Option<PathBuf> {
    locate_install(install_dir, store).map(|i| i.launcher)
}

/// Ghidra's headless launcher in the folder the person picked or the managed install only: what
/// [`headless`] finds before it looks around the computer.
pub fn headless_configured(install_dir: Option<&Path>, store: &Path) -> Option<PathBuf> {
    if let Some(dir) = install_dir {
        let found = if dir.is_file() { launcher_from_file(dir) } else { in_root_or_child(dir, launcher) };
        if found.is_some() {
            return found;
        }
    }
    crate::in_managed(&store.join(ID), host_exes())
}

/// Longest start script read when following one to its Ghidra folder.
const SCRIPT_CAP: u64 = 16 * 1024;
/// Most paths a start script names that are looked at on disk.
const SCRIPT_PATHS: usize = 32;

/// Ghidra's headless launcher from a file the person picked as Ghidra's folder. Whatever is returned
/// is run as the launcher, so it is only ever a file named like Ghidra's own `support/analyzeHeadless`
/// or a short text file that names one by its absolute path (whether it runs it isn't checked):
/// - the file itself when it is that launcher (by name, as given, even through a link), or a script
///   naming that launcher (a wrapper that sets up Java first, say);
/// - else the launcher of the Ghidra folder the file is in, links followed (`ghidraRun` on PATH as
///   a link);
/// - else, for a start script that names Ghidra's folder (`exec /opt/ghidra/ghidraRun "$@"`), the
///   launcher of the Ghidra folder it names. (Arch's package links `/usr/bin/ghidra` to `ghidraRun`
///   instead, which the case above covers.)
///
/// Anything else is `None`, so the setting isn't listed as Ghidra.
fn launcher_from_file(file: &Path) -> Option<PathBuf> {
    let exes = host_exes();
    let is_launcher = |p: &Path| p.file_name().is_some_and(|n| exes.iter().any(|exe| n == *exe));
    let real = std::fs::canonicalize(file).ok()?;
    if is_launcher(file) || is_launcher(&real) {
        return Some(file.to_path_buf());
    }
    // A script that runs the launcher is kept before anything else, even in Ghidra's own folder.
    let named = named_in_script(&real).unwrap_or_default();
    if named.iter().any(|p| is_launcher(p) && p.is_file()) {
        return Some(file.to_path_buf());
    }
    let folder_of = |p: &Path| p.parent().and_then(|dir| launcher(dir).or_else(|| dir.parent().and_then(launcher)));
    // The path as given first, then where its links lead: on Windows `canonicalize` adds `\\?\`,
    // which batch files handle badly.
    let followed = |p: &Path| folder_of(p).or_else(|| folder_of(&std::fs::canonicalize(p).ok()?));
    if let Some(found) = folder_of(file).or_else(|| folder_of(&real)) {
        return Some(found);
    }
    named.iter().find_map(|p| if p.is_dir() { launcher(p) } else { followed(p) })
}

/// The absolute paths a short text file names that are on disk, as a start script names Ghidra's
/// folder (`None` when it is too long or isn't text). A quoted path is taken whole, spaces and all.
/// One level only: a script it names isn't read in turn.
fn named_in_script(file: &Path) -> Option<Vec<PathBuf>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(file).ok()?.take(SCRIPT_CAP + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > SCRIPT_CAP || bytes.contains(&0) {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    // Between quotes first (odd pieces of a split on that quote), then word by word.
    let quoted = ['"', '\''].into_iter().flat_map(|q| text.split(q).skip(1).step_by(2));
    // `$GHIDRA_HOME/ghidraRun` after `GHIDRA_HOME=/opt/ghidra`: the assigned folder is named too.
    let words = text.split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '=' | ';' | '`' | '(' | ')'));
    let mut paths: Vec<PathBuf> = Vec::new();
    for p in quoted.chain(words).map(PathBuf::from).filter(|p| p.is_absolute()) {
        if !paths.contains(&p) {
            paths.push(p);
        }
        if paths.len() == SCRIPT_PATHS {
            break;
        }
    }
    paths.retain(|p| p.exists());
    Some(paths)
}

/// The Java runtime to run Ghidra with: the one the person set (its fitness for Ghidra's version
/// is judged before Ghidra starts, in [`check_java`]), else the managed one with the highest
/// major version that is new enough. `None` lets Ghidra look for its own.
pub fn java_home(java_home: Option<&Path>, store: &Path) -> Option<PathBuf> {
    java_home.filter(|p| p.is_dir()).map(Path::to_path_buf).or_else(|| managed_java(store, JAVA_MAJOR))
}

/// The managed Java with the highest major version that is at least `min`: the guided install
/// unpacks one, and an older one beside it (left by an earlier Ghidra) must never be picked
/// over it — 12.2 on Java 21 fails hours after it starts, or before it does.
fn managed_java(store: &Path, min: u32) -> Option<PathBuf> {
    let jdk = store.join(JAVA_FOLDER);
    let mut homes: Vec<(u32, PathBuf)> =
        std::fs::read_dir(&jdk).ok()?.flatten().map(|e| e.path()).filter_map(|d| java::home_in(&d)).filter_map(|h| java::major(&h).map(|m| (m, h))).collect();
    homes.sort_by(|a, b| {
        b.0.cmp(&a.0).then_with(|| {
            version_parse(&a.1.file_name().unwrap_or_default().to_string_lossy()).cmp(&version_parse(&b.1.file_name().unwrap_or_default().to_string_lossy()))
        })
    });
    homes.into_iter().find(|(m, _)| *m >= min).map(|(_, h)| h)
}

/// Download, check and unpack Java (unless one new enough is already managed) and Ghidra (unless
/// one this new or newer is already managed or outside the store), then point Ghidra at that
/// Java. `configured_java` is the Java set for Ghidra: an outside copy that would run on a
/// too-old Java isn't reused, because only a store install can be pointed at the Java the setup
/// brings. `progress` gets (what, bytes so far, total); a step with no bytes (0, `None`) is
/// setting up. Returns the headless launcher. Only ever writes inside `store`.
pub fn install(store: &Path, configured_java: Option<&Path>, cancel: Option<Cancel>, progress: &dyn Fn(&str, u64, Option<u64>)) -> Result<PathBuf, ToolError> {
    // Ghidra this new or newer is already on this computer, outside our store (PATH, a system
    // package, Homebrew, an unzipped copy in Downloads…): use it rather than download the same
    // release again (#740). An older copy is not reused (the person asked for this release), and
    // neither is one whose Java is known to be too old or missing: writing a Java override into
    // another program's folder isn't ours to do, so the store copy is set up instead.
    let inherited = std::env::var_os("JAVA_HOME").map(PathBuf::from);
    if let Some(found) = locate_install(None, store).filter(|i| reusable_outside(i, store, configured_java, inherited.as_deref())) {
        return Ok(found.launcher);
    }
    let (Some(_), Some(_)) = (pin(ID), pin("java")) else {
        return Err(ToolError::new(ErrorCode::NotSupported, "Ghidra can't be installed for you on this computer.")
            .with_remediation(format!("Install Ghidra {VERSION} from {PAGE}, then choose its folder in Settings → Engines.")));
    };
    let java = ensure_java(store, cancel.clone(), progress)?;
    let exe = ensure_ghidra(store, cancel, progress)?;
    if let Some(support) = exe.parent() {
        point_at_java(&support.join("launch.properties"), &java)?;
    }
    Ok(exe)
}

/// Whether an install outside our store can be used as it is: this release or newer, and a Java
/// its launcher would really run on is good enough. A too-old or missing Java isn't ours to
/// change in another program's folder (`brew`'s, say), so the store copy is set up instead —
/// which is also the recovery "Get Ghidra" offers for a Java too old for an outside Ghidra.
fn reusable_outside(install: &GhidraInstall, store: &Path, configured: Option<&Path>, inherited: Option<&Path>) -> bool {
    !install.launcher.starts_with(store)
        && !is_below_current(install.version_hint.as_ref())
        && !matches!(java_in_use(&install.launcher, configured, inherited), GhidraJava::TooOld { .. } | GhidraJava::NotJava(_))
}

/// A managed Java new enough for Ghidra: the one already there, or a freshly downloaded Temurin
/// 25. A managed Java below the floor (left by an earlier Ghidra) is replaced, not reused.
fn ensure_java(store: &Path, cancel: Option<Cancel>, progress: &dyn Fn(&str, u64, Option<u64>)) -> Result<PathBuf, ToolError> {
    if let Some(home) = managed_java(store, JAVA_MAJOR) {
        return Ok(home);
    }
    let Some(java) = pin("java") else {
        return Err(ToolError::new(ErrorCode::NotSupported, format!("Ghidra {VERSION} needs Java {JAVA_MAJOR}, and none can be installed for you here."))
            .with_remediation(format!("Install a JDK {JAVA_MAJOR} yourself, then choose its folder in Settings → Engines.")));
    };
    fetch(java, store, JAVA_FOLDER, &format!("Java {JAVA_MAJOR} runtime"), cancel, progress)?;
    managed_java(store, JAVA_MAJOR).ok_or_else(|| ToolError::new(ErrorCode::GarbledOutput, "Java was unpacked, but there was no usable Java home in it."))
}

/// The managed Ghidra: the one already unpacked (this version or newer) or a freshly downloaded
/// one. An older managed install is kept as it is and left untouched; this release lands beside
/// it, and the Java that runs it is the one [`ensure_java`] made sure of.
fn ensure_ghidra(store: &Path, cancel: Option<Cancel>, progress: &dyn Fn(&str, u64, Option<u64>)) -> Result<PathBuf, ToolError> {
    if let Some(found) = crate::in_managed(&store.join(ID), host_exes()).filter(|l| !is_below_current(version_hint_of(l).as_ref())) {
        return Ok(found);
    }
    let Some(ghidra) = pin(ID) else {
        return Err(ToolError::new(ErrorCode::NotSupported, "Ghidra can't be installed for you on this computer.")
            .with_remediation(format!("Install Ghidra {VERSION} from {PAGE}, then choose its folder in Settings → Engines.")));
    };
    fetch(ghidra, store, ID, "Ghidra", cancel, progress)?;
    crate::in_managed(&store.join(ID), host_exes())
        .filter(|l| !is_below_current(version_hint_of(l).as_ref()))
        .ok_or_else(|| ToolError::new(ErrorCode::GarbledOutput, "Ghidra was unpacked, but its launcher wasn't where we expected."))
}

/// One pinned archive: download beside the store (hashed as it streams; a mismatch deletes it),
/// unpack into a temporary folder and only then swap it in as `<store>/<folder>`.
fn fetch(pin: &crate::Pin, store: &Path, folder: &str, what: &str, cancel: Option<Cancel>, progress: &dyn Fn(&str, u64, Option<u64>)) -> Result<(), ToolError> {
    std::fs::create_dir_all(store).map_err(|e| ToolError::io(store.display(), e))?;
    let archive = store.join(format!(".{folder}.archive"));
    crate::install::download_verified(pin.url, &archive, pin.sha256, 1 << 30, cancel.as_ref(), &|got, total| progress(what, got, total))?;
    progress(&format!("Setting up {what}"), 0, None);
    let data = std::fs::read(&archive).map_err(|e| ToolError::io(archive.display(), e));
    let _ = std::fs::remove_file(&archive);
    let data = data?;
    let incoming = store.join(format!(".{folder}.incoming"));
    let _ = std::fs::remove_dir_all(&incoming);
    let dest = store.join(folder);
    let swapped = crate::install::unpack(&data, pin.format, &incoming, pin.strip_prefix, cancel.as_ref(), &|_| {}).and_then(|_| swap_in(&incoming, &dest));
    if swapped.is_err() {
        let _ = std::fs::remove_dir_all(&incoming);
    }
    swapped
}

/// Replace only the entries an unpacked release brings; a kept install beside them (an older
/// Ghidra, still usable under the legacy policy) stays untouched.
fn swap_in(incoming: &Path, dest: &Path) -> Result<(), ToolError> {
    std::fs::create_dir_all(dest).map_err(|e| ToolError::io(dest.display(), e))?;
    for from in std::fs::read_dir(incoming).map_err(|e| ToolError::io(incoming.display(), e))?.flatten().map(|e| e.path()).collect::<Vec<_>>() {
        let to = dest.join(from.file_name().unwrap_or_default());
        let _ = std::fs::remove_dir_all(&to);
        let _ = std::fs::remove_file(&to);
        std::fs::rename(&from, &to).map_err(|e| ToolError::io(to.display(), e))?;
    }
    let _ = std::fs::remove_dir_all(incoming);
    Ok(())
}

/// Ghidra's supported way to choose its Java: `JAVA_HOME_OVERRIDE` in support/launch.properties.
fn point_at_java(props: &Path, java: &Path) -> Result<(), ToolError> {
    // Forward slashes: Ghidra reads this file without escapes, and Java takes them on Windows too.
    let line = format!("JAVA_HOME_OVERRIDE={}", java.display().to_string().replace('\\', "/"));
    let old = std::fs::read_to_string(props).unwrap_or_default();
    let mut lines: Vec<String> = old.lines().filter(|l| !l.trim_start().starts_with("JAVA_HOME_OVERRIDE")).map(str::to_string).collect();
    lines.push(line);
    std::fs::write(props, lines.join("\n") + "\n").map_err(|e| ToolError::io(props.display(), e))
}

/// The Java Ghidra will run on, found the way its launcher looks: a working `JAVA_HOME_OVERRIDE`
/// in its `support/launch.properties` first, then the `JAVA_HOME` it's started with, then a `java`
/// on PATH — each judged against the floor the launcher's own version needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhidraJava {
    /// `JAVA_HOME_OVERRIDE` in Ghidra's launch.properties (a guided install writes it).
    Override(PathBuf),
    /// The `JAVA_HOME` Ghidra is started with: the one set for it (`configured`), or the one this
    /// program was started with.
    Home { dir: PathBuf, configured: bool },
    /// A `java` on PATH.
    OnPath(PathBuf),
    /// The folder set as Ghidra's Java has no `bin/java`.
    NotJava(PathBuf),
    /// A Java whose version is known and below what this Ghidra needs: Ghidra is stopped before
    /// it starts, with the versions spelled out.
    TooOld { dir: PathBuf, found: u32, needed: u32 },
    /// A Java with a `bin/java` whose version can't be read: Ghidra gets to judge it.
    Unknown(PathBuf),
    /// No Java at all: Ghidra looks on its own paths (next to a Homebrew install, say).
    Missing,
}

/// Which Java the Ghidra at `launcher` will run on, and whether it is new enough: the floor comes
/// from the launcher's own version (25 for this release, 21 for a kept legacy install).
/// `configured` is the Java set for it (`Settings::ghidra_java`), `inherited` the `JAVA_HOME`
/// this program has.
pub fn java_in_use(launcher: &Path, configured: Option<&Path>, inherited: Option<&Path>) -> GhidraJava {
    let floor = java_floor(version_hint_of(launcher).as_ref());
    // Ghidra's launcher follows a link to itself (one on PATH) and reads the properties beside
    // the real one.
    let launcher = std::fs::canonicalize(launcher).unwrap_or_else(|_| launcher.to_path_buf());
    let props = launcher.parent().map(|d| d.join("launch.properties"));
    let over = props.and_then(|p| std::fs::read_to_string(p).ok()).and_then(|text| java_override(&text, &|k| std::env::var(k).ok()));
    if let Some(dir) = over.filter(|d| has_java(d)) {
        return judged_java(GhidraJava::Override(dir), floor);
    }
    match configured {
        Some(dir) if has_java(dir) => return judged_java(GhidraJava::Home { dir: dir.to_path_buf(), configured: true }, floor),
        Some(dir) => return GhidraJava::NotJava(dir.to_path_buf()),
        None => {}
    }
    if let Some(dir) = inherited.filter(|d| has_java(d)) {
        return judged_java(GhidraJava::Home { dir: dir.to_path_buf(), configured: false }, floor);
    }
    match crate::find_on_path(&[if cfg!(windows) { "java.exe" } else { "java" }]) {
        Some(p) => judged_java(GhidraJava::OnPath(p), floor),
        None => GhidraJava::Missing,
    }
}

/// A Java in one of the `has a bin/java` branches, judged against the floor it must meet: a
/// known version at or above the floor keeps its branch, a known lower one becomes `TooOld`,
/// an unreadable one `Unknown`.
fn judged_java(java: GhidraJava, floor: u32) -> GhidraJava {
    let home = match &java {
        GhidraJava::Override(d) | GhidraJava::Home { dir: d, .. } => Some(d.clone()),
        GhidraJava::OnPath(p) => p.parent().and_then(Path::parent).map(Path::to_path_buf),
        _ => None,
    };
    let Some(home) = home else { return java };
    match java::verdict(&home, floor) {
        JavaVerdict::Ok(_) => java,
        JavaVerdict::TooOld(found) => GhidraJava::TooOld { dir: home, found, needed: floor },
        JavaVerdict::Unknown => GhidraJava::Unknown(home),
    }
}

/// Stops before Ghidra starts when its Java is known to be too old or the folder set as its Java
/// holds none, or the chosen Ghidra is no longer supported: failing here, with the versions
/// spelled out, beats failing hours into an analysis. Which Java it would run on is judged the
/// way [`java_in_use`] finds it, so this check and the status every listing shows agree — a
/// `JAVA_HOME_OVERRIDE` in a store install wins over a too-old Java set in Settings. A Java whose
/// version can't be read and no Java at all are left for Ghidra to judge on its own paths. Every
/// way Ghidra is started goes through [`super::analyse`], so UI, CLI, MCP and the build agent all
/// get the same stop.
pub fn check_java(cfg: &Config) -> Result<(), GhidraError> {
    let below = is_below_current(version_hint_of(&cfg.analyze_headless).as_ref());
    if below && LEGACY_POLICY == LegacyPolicy::Refuse {
        return Err(GhidraError::Launch(format!(
            "Ghidra {VERSION} is the supported version, and the one chosen ({}) isn't supported any more. Install Ghidra {VERSION} from Settings → Engines.",
            cfg.analyze_headless.display()
        )));
    }
    let inherited = std::env::var_os("JAVA_HOME").map(PathBuf::from);
    match java_in_use(&cfg.analyze_headless, cfg.java_home.as_deref(), inherited.as_deref()) {
        GhidraJava::TooOld { dir, found, needed } => Err(GhidraError::Launch(format!(
            "Ghidra needs Java {needed}, but the Java it would run on is Java {found} ({}). Choose a newer Java in Settings → Engines, or clear the setting to use the one the guided install brings.",
            dir.display()
        ))),
        GhidraJava::NotJava(dir) => Err(GhidraError::Launch(format!(
            "The folder set as Ghidra's Java ({}) has no bin/java, so Ghidra can't start. Choose a JDK folder in Settings → Engines, or clear the setting to use the one the guided install brings.",
            dir.display()
        ))),
        // Every other branch is either good enough or Ghidra's to judge when it starts.
        _ => Ok(()),
    }
}

/// The `JAVA_HOME_OVERRIDE` in Ghidra's launch.properties text, read the way Ghidra does: `KEY=value`
/// with or without spaces around the `=`, the first one with a value, and `${NAME}` replaced by
/// that environment variable (`env`; an unset one becomes nothing).
fn java_override(text: &str, env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let value =
        text.lines().filter_map(|l| l.split_once('=')).filter(|(k, _)| k.trim() == "JAVA_HOME_OVERRIDE").map(|(_, v)| v.trim()).find(|v| !v.is_empty())?;
    let mut out = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        out.push_str(rest.get(..start).unwrap_or_default());
        let after = rest.get(start + 2..).unwrap_or_default();
        let Some(end) = after.find('}') else {
            // No closing brace: the rest is taken as it is.
            out.push_str(rest.get(start..).unwrap_or_default());
            rest = "";
            break;
        };
        out.push_str(&env(after.get(..end).unwrap_or_default()).unwrap_or_default());
        rest = after.get(end + 1..).unwrap_or_default();
    }
    out.push_str(rest);
    Some(PathBuf::from(out)).filter(|p| !p.as_os_str().is_empty())
}

fn launcher(root: &Path) -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) { &["analyzeHeadless.bat"] } else { &["analyzeHeadless"] };
    names.iter().map(|n| root.join("support").join(n)).find(|p| p.is_file())
}

fn has_java(dir: &Path) -> bool {
    dir.join("bin").join(if cfg!(windows) { "java.exe" } else { "java" }).is_file()
}

/// `find` on `dir` itself, else on its sub-folders (a zip unpacks into one top folder).
fn in_root_or_child(dir: &Path, find: impl Fn(&Path) -> Option<PathBuf>) -> Option<PathBuf> {
    find(dir).or_else(|| in_child(dir, find))
}

fn in_child(dir: &Path, find: impl Fn(&Path) -> Option<PathBuf>) -> Option<PathBuf> {
    let mut subs: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    subs.sort_by(|a, b| version_order(&a.file_name().unwrap_or_default().to_string_lossy(), &b.file_name().unwrap_or_default().to_string_lossy()));
    subs.iter().rev().find_map(|d| find(d))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::JDK_PINNED;

    fn touch(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"x").unwrap();
    }

    /// A JDK folder a test can point at: `bin/java` and the `release` file a version is read from.
    fn jdk(at: &Path, version: &str) -> PathBuf {
        let home = at.to_path_buf();
        touch(&home.join("bin").join(if cfg!(windows) { "java.exe" } else { "java" }));
        std::fs::write(home.join("release"), format!("JAVA_VERSION=\"{version}\"\n")).unwrap();
        home
    }

    #[test]
    fn a_macos_jdk_unpacks_to_a_java_home_we_find() {
        use craft_anything_testkit::firmware as fx;
        let pin = crate::engine("java").and_then(|e| e.pin_for("macos", "aarch64")).unwrap();
        let java = if cfg!(windows) { "java.exe" } else { "java" };
        let top = format!("jdk-{JDK_PINNED}/Contents/");
        let tar = fx::tar_entries(&[
            (&format!("{top}Info.plist"), b'0', b"<plist/>", ""),
            (&format!("{top}Home/bin/{java}"), b'0', b"java", ""),
            (&format!("{top}Home/lib/modules"), b'0', b"m", ""),
            (&format!("{top}Home/release"), b'0', b"JAVA_VERSION=\"25.0.1\"\n", ""),
        ]);
        let store = tempfile::tempdir().unwrap();
        let jdk_home = store.path().join("jdk");
        crate::install::unpack(&fx::gzip(&tar, None), pin.format, &jdk_home, pin.strip_prefix, None, &|_| {}).unwrap();
        assert_eq!(java_home(None, store.path()), Some(jdk_home.join("Home")));
        let linux = crate::engine("java").and_then(|e| e.pin_for("linux", "x86_64")).unwrap();
        assert_eq!((linux.format, linux.strip_prefix), (crate::ArchiveFormat::TarGz, None), "Linux keeps jdk-…/bin/java, one folder down");
    }

    /// Ghidra's folder holds both scripts; the `.bat` can't start off Windows (#1084).
    #[test]
    fn the_launcher_looked_for_on_path_is_this_systems_own() {
        let exes = crate::engine(ID).unwrap().host_exes();
        assert_eq!(exes, [if cfg!(windows) { "analyzeHeadless.bat" } else { "analyzeHeadless" }]);
    }

    #[test]
    fn finds_ghidra_and_java_where_they_really_live() {
        let d = tempfile::tempdir().unwrap();
        let store = d.path().join("store");
        let exe = if cfg!(windows) { "analyzeHeadless.bat" } else { "analyzeHeadless" };
        // None, unless this computer has Ghidra on PATH, from a system package, or from Homebrew.
        let e = crate::engine(ID).unwrap();
        let installed = crate::find_on_path(&[exe]).is_some() || crate::well_known(ID).iter().any(|p| p.is_file()) || crate::find_in_package_paths(e).is_some();
        assert!(headless(None, &store).is_none() || installed);
        // The managed install: the zip's own top folder under <store>/ghidra.
        let managed = store.join("ghidra/ghidra_12.2_PUBLIC/support").join(exe);
        touch(&managed);
        assert_eq!(headless(None, &store), Some(managed));
        // A folder the person picked wins, whether it's Ghidra's folder or the one holding it.
        let own = d.path().join("tools/ghidra_11.4/support").join(exe);
        touch(&own);
        assert_eq!(headless(Some(&d.path().join("tools/ghidra_11.4")), &store), Some(own.clone()));
        assert_eq!(headless(Some(&d.path().join("tools")), &store), Some(own));

        assert!(java_home(None, &store).is_none());
        let jdk_dir = jdk(&store.join("jdk").join("jdk-25.0.5.1+2"), "25.0.1");
        assert_eq!(java_home(None, &store), Some(jdk_dir));
        assert_eq!(java_home(Some(d.path()), &store).as_deref(), Some(d.path()), "a Java the person set wins");
    }

    const EXE: &str = if cfg!(windows) { "analyzeHeadless.bat" } else { "analyzeHeadless" };

    /// A Ghidra folder as its zip unpacks: `ghidraRun` at the top, the headless launcher in `support/`.
    fn ghidra_folder(at: &Path) -> PathBuf {
        touch(&at.join("ghidraRun"));
        let exe = at.join("support").join(EXE);
        touch(&exe);
        exe
    }

    fn same(a: Option<PathBuf>, b: &Path) -> bool {
        a.and_then(|a| std::fs::canonicalize(a).ok()) == std::fs::canonicalize(b).ok()
    }

    /// A Ghidra setting that names a file is run as Ghidra's headless launcher, so it has to be one.
    #[test]
    fn a_setting_naming_the_launcher_itself_is_that_launcher() {
        let d = tempfile::tempdir().unwrap();
        let exe = ghidra_folder(&d.path().join("ghidra_12.2_PUBLIC"));
        assert_eq!(headless_configured(Some(&exe), &d.path().join("store")), Some(exe));
    }

    /// Arch's package puts a start script on PATH that runs `/opt/ghidra/ghidraRun`, Ghidra's window,
    /// not its headless launcher: picked as the setting, it was listed as found and Deep mode
    /// quietly used the built-in decompiler.
    #[test]
    fn a_start_script_is_followed_to_the_ghidra_folder_it_names() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("opt").join("My Ghidra");
        let exe = ghidra_folder(&root);
        let script = d.path().join("bin").join("ghidra");
        let store = d.path().join("store");
        let mut bodies = vec![
            format!("#!/bin/sh\nexec \"{}\" \"$@\"\n", root.join("ghidraRun").display()),
            format!("#!/bin/sh\nGHIDRA_HOME='{}'\nexec \"$GHIDRA_HOME/ghidraRun\" \"$@\"\n", root.display()),
        ];
        let plain = d.path().join("opt").join("ghidra");
        if !plain.display().to_string().contains(' ') {
            ghidra_folder(&plain);
            bodies.push(format!("#!/bin/sh\nexec {}/ghidraRun \"$@\"\n", plain.display()));
        }
        for body in bodies {
            touch(&script);
            std::fs::write(&script, &body).unwrap();
            let found = headless_configured(Some(&script), &store);
            assert!(same(found.clone(), &exe) || same(found, &plain.join("support").join(EXE)), "{body}");
            assert!(!headless_configured(Some(&script), &store).unwrap().display().to_string().starts_with(r"\\?\"));
        }
        // Too long to be a start script: not read.
        std::fs::write(&script, format!("{}\nexec \"{}\"\n", "#".repeat(SCRIPT_CAP as usize), root.join("ghidraRun").display())).unwrap();
        assert_eq!(headless_configured(Some(&script), &store), None);
    }

    /// A script that runs the headless launcher itself (setting up Java first, say) is run as it is.
    #[test]
    fn a_wrapper_around_the_launcher_is_kept() {
        let d = tempfile::tempdir().unwrap();
        let exe = ghidra_folder(&d.path().join("ghidra"));
        let wrapper = d.path().join("bin").join("ghidra-headless");
        touch(&wrapper);
        std::fs::write(&wrapper, format!("#!/bin/sh\nJAVA_HOME=/jdk exec \"{}\" \"$@\"\n", exe.display())).unwrap();
        assert_eq!(headless_configured(Some(&wrapper), &d.path().join("store")), Some(wrapper));
        // Also when it sits in Ghidra's own folder, beside `ghidraRun`.
        let beside = d.path().join("ghidra").join("headless-with-java");
        std::fs::write(&beside, format!("#!/bin/sh\nJAVA_HOME=/jdk exec \"{}\" \"$@\"\n", exe.display())).unwrap();
        assert_eq!(headless_configured(Some(&beside), &d.path().join("store")), Some(beside));
    }

    /// A start script that runs a link onto PATH (`exec /usr/bin/ghidraRun`) leads to where it points.
    #[cfg(unix)]
    #[test]
    fn a_start_script_naming_a_link_is_followed_through_it() {
        let d = tempfile::tempdir().unwrap();
        let exe = ghidra_folder(&d.path().join("opt").join("ghidra"));
        let bin = d.path().join("usr").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink(d.path().join("opt").join("ghidra").join("ghidraRun"), bin.join("ghidraRun")).unwrap();
        let script = d.path().join("ghidra");
        std::fs::write(&script, format!("#!/bin/sh\nexec \"{}\" \"$@\"\n", bin.join("ghidraRun").display())).unwrap();
        assert!(same(headless_configured(Some(&script), &d.path().join("store")), &exe));
    }

    #[test]
    fn a_file_that_leads_to_no_ghidra_is_not_ghidra() {
        let d = tempfile::tempdir().unwrap();
        let store = d.path().join("store");
        let notes = d.path().join("notes.txt");
        std::fs::write(&notes, "nothing to see").unwrap();
        assert_eq!(headless_configured(Some(&notes), &store), None);
        let script = d.path().join("ghidra");
        std::fs::write(&script, format!("#!/bin/sh\nexec {}/ghidraRun \"$@\"\n", d.path().join("gone").display())).unwrap();
        assert_eq!(headless_configured(Some(&script), &store), None);
        std::fs::write(&script, [0xff, 0xfe, 0x00, 0x2f]).unwrap();
        assert_eq!(headless_configured(Some(&script), &store), None, "not text");
        // With nothing usable set, the managed install is still found.
        let managed = ghidra_folder(&store.join(ID).join("ghidra_12.2_PUBLIC"));
        assert_eq!(headless_configured(Some(&notes), &store), Some(managed));
    }

    /// Homebrew and some packages link `ghidraRun` onto PATH instead.
    #[cfg(unix)]
    #[test]
    fn a_link_to_ghidras_start_script_is_followed_to_its_folder() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("Caskroom").join("ghidra_12.2_PUBLIC");
        let exe = ghidra_folder(&root);
        let bin = d.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink(root.join("ghidraRun"), bin.join("ghidraRun")).unwrap();
        assert!(same(headless_configured(Some(&bin.join("ghidraRun")), &d.path().join("store")), &exe));
        std::os::unix::fs::symlink(&exe, bin.join(EXE)).unwrap();
        assert_eq!(headless_configured(Some(&bin.join(EXE)), &d.path().join("store")), Some(bin.join(EXE)), "a linked launcher is kept as given");
    }

    #[test]
    fn the_java_override_is_read_as_ghidra_reads_it() {
        let env = |k: &str| (k == "JDK_HOME").then(|| "/opt/jdk-25".to_string());
        let read = |t: &str| java_override(t, &env);
        assert_eq!(read("JAVA_HOME_OVERRIDE=/a\nJAVA_HOME_OVERRIDE=/b\n"), Some(PathBuf::from("/a")), "the first one");
        assert_eq!(read("JAVA_HOME_OVERRIDE=\nJAVA_HOME_OVERRIDE = /b\n"), Some(PathBuf::from("/b")), "the first with a value");
        assert_eq!(read("JAVA_HOME_OVERRIDE=${JDK_HOME}\n"), Some(PathBuf::from("/opt/jdk-25")));
        assert_eq!(read("JAVA_HOME_OVERRIDE=${JDK_HOME}/x/${NOPE}y\n"), Some(PathBuf::from("/opt/jdk-25/x/y")));
        assert_eq!(read("JAVA_HOME_OVERRIDE=${NOPE}\n"), None, "an unset variable leaves no path");
        assert_eq!(read("JAVA_HOME_OVERRIDE=/a${JDK\n"), Some(PathBuf::from("/a${JDK")));
        assert_eq!(read("VMARGS=-Xshare:off\n"), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_launcher_reached_through_a_link_reads_the_real_installs_properties() {
        let d = tempfile::tempdir().unwrap();
        let support = d.path().join("ghidra").join("support");
        let jdk_home = jdk(&d.path().join("jdk"), "25.0.1");
        std::fs::create_dir_all(&support).unwrap();
        std::fs::write(support.join("analyzeHeadless"), "").unwrap();
        std::fs::write(support.join("launch.properties"), format!("JAVA_HOME_OVERRIDE={}\n", jdk_home.display())).unwrap();
        let bin = d.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink(support.join("analyzeHeadless"), bin.join("analyzeHeadless")).unwrap();
        let canon = std::fs::canonicalize(&jdk_home).unwrap();
        assert!(matches!(java_in_use(&bin.join("analyzeHeadless"), None, None), GhidraJava::Override(p) if p == jdk_home || p == canon));
    }

    #[test]
    fn ghidra_is_pointed_at_our_java() {
        let d = tempfile::tempdir().unwrap();
        let props = d.path().join("launch.properties");
        std::fs::write(&props, "VMARGS=-Xshare:off\nJAVA_HOME_OVERRIDE=\n").unwrap();
        point_at_java(&props, Path::new(r"C:\store\jdk\jdk-25")).unwrap();
        let text = std::fs::read_to_string(&props).unwrap();
        assert_eq!(text, "VMARGS=-Xshare:off\nJAVA_HOME_OVERRIDE=C:/store/jdk/jdk-25\n");
    }

    #[test]
    fn pins_come_from_the_engine_table() {
        // Checksum handling is tested once, in `install` (download_verified).
        let ghidra = crate::engine("ghidra").unwrap().pins();
        assert!(ghidra.iter().all(|p| p.url.contains(VERSION)));
        assert!(version_parse(VERSION).is_some(), "VERSION names a version: min_current depends on it");
        let java_pinned = cfg!(any(
            all(windows, target_arch = "x86_64"),
            all(any(target_os = "linux", target_os = "macos"), any(target_arch = "x86_64", target_arch = "aarch64"))
        ));
        assert_eq!(can_install(), java_pinned);
    }

    /// Versions are read out of folder names, a few levels up from the launcher: a release's own
    /// top folder, Homebrew's `Cellar` version folder; a folder without a version names none.
    #[test]
    fn the_version_hint_comes_from_the_folder_name() {
        let exe = if cfg!(windows) { "analyzeHeadless.bat" } else { "analyzeHeadless" };
        let store = tempfile::tempdir().unwrap();
        let launcher = store.path().join("ghidra/ghidra_12.2_PUBLIC/support").join(exe);
        touch(&launcher);
        let install = locate_install(None, store.path()).unwrap();
        assert_eq!(install.version_hint, Some(Version(vec![12, 2])));
        assert_eq!(install.source, GhidraSource::EngineStore);
        // Homebrew's layout carries the version two folders higher up.
        let d = tempfile::tempdir().unwrap();
        let brew = d.path().join("homebrew");
        let rel = "libexec/support/analyzeHeadless";
        touch(&brew.join("Cellar/ghidra/12.2").join(rel));
        let hint = version_hint_of(&brew.join("Cellar/ghidra/12.2").join(rel));
        assert_eq!(hint, Some(Version(vec![12, 2])));
        // A name without a version stays without one.
        let plain = d.path().join("opt/ghidra/support").join(exe);
        touch(&plain);
        assert_eq!(version_hint_of(&plain), None);
        // A number in an unrelated parent's name is not a version (`/home/user1/ghidra/…`).
        let user = d.path().join("home/user1/ghidra/support").join(exe);
        touch(&user);
        assert_eq!(version_hint_of(&user), None);
    }

    /// Homebrew links `opt/ghidra` to `Cellar/ghidra/<version>`: the version is in the real path,
    /// so the linked path gets it too.
    #[cfg(unix)]
    #[test]
    fn a_linked_launcher_takes_its_version_from_where_it_leads() {
        let d = tempfile::tempdir().unwrap();
        let rel = "libexec/support/analyzeHeadless";
        let cellar = d.path().join("Cellar/ghidra/12.2").join(rel);
        touch(&cellar);
        std::fs::create_dir_all(d.path().join("opt")).unwrap();
        // `opt/ghidra` stands in for the linked package folder; the launcher is reached through it.
        std::os::unix::fs::symlink(d.path().join("Cellar/ghidra/12.2"), d.path().join("opt/ghidra")).unwrap();
        let linked = d.path().join("opt/ghidra").join(rel);
        assert_eq!(version_hint_of(&linked), Some(Version(vec![12, 2])));
        // The launch uses the path as given; only the version is read through the link.
        let install = install_of(linked.clone(), GhidraSource::PackageManager("Homebrew"));
        assert_eq!(install.launcher, linked);
        assert_eq!(install.version_hint, Some(Version(vec![12, 2])));
    }

    /// Two managed installs side by side: the newest wins, by the numbers. `12.10.0` beats
    /// `12.2`, which plain text order would get backwards, and a name without a version never
    /// stops the search.
    #[test]
    fn when_versions_coexist_the_newest_is_chosen() {
        let exe = if cfg!(windows) { "analyzeHeadless.bat" } else { "analyzeHeadless" };
        let store = tempfile::tempdir().unwrap();
        let old = store.path().join("ghidra/ghidra_12.1.4_PUBLIC/support").join(exe);
        let new = store.path().join("ghidra/ghidra_12.2_PUBLIC/support").join(exe);
        touch(&old);
        touch(&new);
        assert_eq!(headless(None, store.path()), Some(new.clone()));
        // A Java 25 home set for Ghidra, so the verdict doesn't depend on the host's own Java.
        let java = jdk(&store.path().join("jdk-25"), "25.0");
        let status = status(None, Some(&java), store.path());
        assert_eq!(status.state, GhidraState::Ready, "{:?}", status.java);
        assert_eq!(status.install.as_ref().unwrap().version_hint, Some(Version(vec![12, 2])));
        // `12.10.0` is newer than `12.2`, not the other way round.
        let newest = store.path().join("ghidra/ghidra_12.10.0_PUBLIC/support").join(exe);
        touch(&newest);
        assert_eq!(headless(None, store.path()), Some(newest.clone()));
        // A custom name beside them: sorted as text, never a crash, never chosen over a number.
        let custom = store.path().join("ghidra/ghidra-custom/support").join(exe);
        touch(&custom);
        assert_eq!(headless(None, store.path()), Some(newest));
    }

    /// A folder the person picked that holds two releases: the newest wins by the numbers there
    /// too (`12.10.0` beats `12.2`, which text order would get backwards).
    #[test]
    fn a_picked_folder_with_two_releases_picks_the_newest_by_the_numbers() {
        let d = tempfile::tempdir().unwrap();
        let newest = ghidra_folder(&d.path().join("ghidra_12.10.0_PUBLIC"));
        let _ = ghidra_folder(&d.path().join("ghidra_12.2_PUBLIC"));
        assert_eq!(headless(Some(d.path()), &d.path().join("store")), Some(newest));
    }

    /// Swapping a release in replaces only what it brings: a kept legacy install beside it stays.
    #[test]
    fn swapping_a_release_in_keeps_a_legacy_install_beside_it() {
        let d = tempfile::tempdir().unwrap();
        let dest = d.path().join("ghidra");
        let legacy = dest.join("ghidra_12.1.4_PUBLIC/support");
        touch(&legacy.join(EXE));
        touch(&dest.join("ghidra_12.2_PUBLIC/support").join(EXE));
        let incoming = d.path().join("incoming");
        touch(&incoming.join("ghidra_12.2_PUBLIC/support").join(EXE));
        swap_in(&incoming, &dest).unwrap();
        assert!(legacy.join(EXE).is_file(), "the kept legacy install is untouched");
        assert!(dest.join("ghidra_12.2_PUBLIC/support").join(EXE).is_file());
        assert!(!incoming.exists());
    }

    /// The Java floor follows the Ghidra version: this release needs 25, a kept legacy install
    /// may run on 21, and an unknown version gets the current floor rather than a guess.
    #[test]
    fn the_java_floor_follows_the_ghidra_version() {
        assert_eq!(java_floor(Some(&Version(vec![12, 2]))), JAVA_MAJOR);
        assert_eq!(java_floor(Some(&Version(vec![12, 10, 0]))), JAVA_MAJOR);
        assert_eq!(java_floor(None), JAVA_MAJOR);
        assert_eq!(java_floor(Some(&Version(vec![12, 1, 4]))), LEGACY_JAVA_MAJOR);
    }

    /// A managed Java below the floor is not picked, whatever its name sorts as; the newest
    /// major version that is new enough wins.
    #[test]
    fn a_managed_java_below_the_floor_is_not_picked() {
        let store = tempfile::tempdir().unwrap();
        jdk(&store.path().join("jdk").join("jdk-21.0.12.1+1"), "21.0.5");
        assert_eq!(managed_java(store.path(), JAVA_MAJOR), None, "the old managed Java is refused");
        let twenty_five = jdk(&store.path().join("jdk").join("jdk-25.0.5.1+2"), "25.0.1");
        assert_eq!(managed_java(store.path(), JAVA_MAJOR), Some(twenty_five.clone()));
        // A newer major version beside it wins over the pinned one.
        let twenty_six = jdk(&store.path().join("jdk").join("jdk-26.0.1"), "26.0.1");
        assert_eq!(managed_java(store.path(), JAVA_MAJOR), Some(twenty_six));
        // The old Java is still there, untouched (other Java tools keep using it).
        let exe = if cfg!(windows) { "java.exe" } else { "java" };
        assert!(store.path().join("jdk").join("jdk-21.0.12.1+1").join("bin").join(exe).is_file());
        let _ = twenty_five;
    }

    /// The status tells the states apart: nothing anywhere is `NotFound`; a folder with no
    /// launcher is `Incomplete`; a legacy install is kept (`Legacy`) and reports its real version;
    /// a too-old Java stops Deep mode before Ghidra starts (`JavaTooOld`).
    #[test]
    fn the_status_tells_the_states_apart() {
        let d = tempfile::tempdir().unwrap();
        let store = d.path().join("store");
        let exe = if cfg!(windows) { "analyzeHeadless.bat" } else { "analyzeHeadless" };
        // Nothing set, nothing found — unless this computer has Ghidra outside the store.
        let e = crate::engine(ID).unwrap();
        let outside = crate::find_on_path(&[exe]).is_some() || crate::well_known(ID).iter().any(|p| p.is_file()) || crate::find_in_package_paths(e).is_some();
        let none = status(None, None, &store);
        if !outside {
            assert_eq!(none.state, GhidraState::NotFound, "{none:?}");
        }
        // A folder that is there but has no launcher is incomplete, not missing. (On a computer
        // with Ghidra somewhere else — PATH, a package, Homebrew — that one is found first, so
        // this asserts only where nothing outside is in the way.)
        std::fs::create_dir_all(store.join("ghidra/ghidra_12.2_PUBLIC")).unwrap();
        if !outside {
            assert_eq!(status(None, None, &store).state, GhidraState::Incomplete);
        }
        // Ready: a launcher and a Java good enough for it.
        let launcher = ghidra_folder(&store.join("ghidra/ghidra_12.2_PUBLIC"));
        let good = jdk(&d.path().join("jdk-25"), "25.0.1");
        assert_eq!(status(None, Some(&good), &store).state, GhidraState::Ready);
        let _ = launcher;
        // A Java too old for this release stops it before it starts.
        let old = jdk(&d.path().join("jdk-21"), "21.0.5");
        let too_old = status(None, Some(&old), &store);
        assert_eq!(too_old.state, GhidraState::JavaTooOld, "{too_old:?}");
        assert!(matches!(too_old.java, GhidraJava::TooOld { found: 21, needed: 25, .. }), "{:?}", too_old.java);
        // A kept legacy install stays usable and says its real version.
        let legacy_store = d.path().join("legacy-store");
        touch(&legacy_store.join("ghidra/ghidra_12.1.4_PUBLIC/support").join(exe));
        let legacy = status(None, Some(&old), &legacy_store);
        assert_eq!(legacy.state, GhidraState::Legacy, "{legacy:?}");
        assert_eq!(legacy.install.as_ref().unwrap().version_hint, Some(Version(vec![12, 1, 4])));
    }

    /// `check_java` stops a known-too-old Java before Ghidra starts, and lets an unreadable one
    /// or no override through for Ghidra to judge.
    #[test]
    fn check_java_stops_a_known_too_old_java() {
        let d = tempfile::tempdir().unwrap();
        let exe = if cfg!(windows) { "analyzeHeadless.bat" } else { "analyzeHeadless" };
        let launcher = d.path().join("ghidra_12.2_PUBLIC/support").join(exe);
        touch(&launcher);
        let old = jdk(&d.path().join("jdk-21"), "21.0.5");
        let good = jdk(&d.path().join("jdk-25"), "25.0.1");
        let cfg = |java: Option<&Path>| Config {
            analyze_headless: launcher.clone(),
            java_home: java.map(Path::to_path_buf),
            startup_timeout: std::time::Duration::from_secs(60),
            store_dir: d.path().join("store"),
            language_id: None,
            compiler_spec: None,
            memory_gb: Some(8),
        };
        assert!(check_java(&cfg(Some(&old))).is_err(), "Java 21 must not run Ghidra 12.2");
        let err = check_java(&cfg(Some(&old))).unwrap_err().to_string();
        assert!(err.contains("25") && err.contains("21"), "{err}");
        assert!(check_java(&cfg(Some(&good))).is_ok());
        // With no Java set, the Java Ghidra would fall back to is judged the same way the status
        // judges it (inherited `JAVA_HOME`, else PATH), so this follows that verdict.
        let fallback = java_in_use(&cfg(None).analyze_headless, None, std::env::var_os("JAVA_HOME").map(PathBuf::from).as_deref());
        assert_eq!(check_java(&cfg(None)).is_ok(), !matches!(fallback, GhidraJava::TooOld { .. } | GhidraJava::NotJava(_)));
        let unreadable = d.path().join("jdk-broken");
        touch(&unreadable.join("bin").join(if cfg!(windows) { "java.exe" } else { "java" }));
        assert!(check_java(&cfg(Some(&unreadable))).is_ok());
        // A legacy install runs on the older floor.
        let legacy = d.path().join("ghidra_12.1.4_PUBLIC/support").join(exe);
        touch(&legacy);
        assert!(check_java(&cfg(Some(&old))).is_err(), "the launcher above is still 12.2");
        let legacy_cfg = Config { analyze_headless: legacy, ..cfg(Some(&old)) };
        assert!(check_java(&legacy_cfg).is_ok(), "12.1.4 kept under the soft policy runs on Java 21");
    }

    /// A store install's `JAVA_HOME_OVERRIDE` is what Ghidra really runs on: it wins over a Java
    /// set in Settings that would be too old, so the launch check and the status agree on the
    /// Java the setup pointed Ghidra at.
    #[test]
    fn a_java_override_wins_over_a_too_old_setting() {
        let d = tempfile::tempdir().unwrap();
        let exe = if cfg!(windows) { "analyzeHeadless.bat" } else { "analyzeHeadless" };
        let support = d.path().join("ghidra_12.2_PUBLIC/support");
        let launcher = support.join(exe);
        touch(&launcher);
        let old = jdk(&d.path().join("jdk-21"), "21.0.5");
        let good = jdk(&d.path().join("jdk-25"), "25.0.1");
        std::fs::write(support.join("launch.properties"), format!("JAVA_HOME_OVERRIDE={}\n", good.display())).unwrap();
        let cfg = Config {
            analyze_headless: launcher.clone(),
            java_home: Some(old.clone()),
            startup_timeout: std::time::Duration::from_secs(60),
            store_dir: d.path().join("store"),
            language_id: None,
            compiler_spec: None,
            memory_gb: Some(8),
        };
        assert!(check_java(&cfg).is_ok(), "the override is the Java Ghidra starts with");
        assert!(matches!(java_in_use(&launcher, Some(&old), None), GhidraJava::Override(dir) if dir == good || dir == std::fs::canonicalize(&good).unwrap()));
    }

    /// An outside Ghidra is reused only when it can really run: this release or newer, and on a
    /// Java its launcher would run on. One on a too-old Java is left alone, so "Get Ghidra" sets
    /// up the store copy with the Java the setup brings instead.
    #[test]
    fn only_an_outside_ghidra_that_can_run_is_reused() {
        let d = tempfile::tempdir().unwrap();
        let store = d.path().join("store");
        let good = jdk(&d.path().join("jdk-25"), "25.0");
        let old = jdk(&d.path().join("jdk-21"), "21.0");
        let outside = install_of(ghidra_folder(&d.path().join("elsewhere/ghidra_12.2_PUBLIC")), GhidraSource::UnpackedFolder);
        assert!(reusable_outside(&outside, &store, Some(&good), None));
        assert!(!reusable_outside(&outside, &store, Some(&old), None), "a too-old Java is not ours to change outside the store");
        // A store install is the managed path, never reused as an outside one.
        let inside = install_of(ghidra_folder(&store.join("ghidra/ghidra_12.2_PUBLIC")), GhidraSource::EngineStore);
        assert!(!reusable_outside(&inside, &store, Some(&good), None));
        // Older than this release: not reused.
        let legacy = install_of(ghidra_folder(&d.path().join("elsewhere/ghidra_12.1.4_PUBLIC")), GhidraSource::UnpackedFolder);
        assert!(!reusable_outside(&legacy, &store, Some(&good), None));
    }
}
