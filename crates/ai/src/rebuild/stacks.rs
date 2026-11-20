//! The stacks a build can use, from the `stacks.toml` table: what the builder is told for each,
//! and the phrases that ask for one by name. Kinds name their default stack; the table says how
//! to build with it. The table also lists libraries asked for on top of a stack (shadcn/ui,
//! Tailwind CSS), whose steps are added to the stack's.

use craft_anything_engine::toolchains::{self, SourceScan};
use craft_anything_engine::workspace::Manifest;
use craft_anything_engines::game;
use std::sync::OnceLock;

/// One stack ("React + TypeScript (Vite)", "Unity"…).
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Stack {
    /// Shown to the person and stored in the project.
    pub name: String,
    /// The game engine it builds with (`unity`, `unreal`, `godot`, `rust`), if any.
    #[serde(default)]
    pub engine: Option<String>,
    /// Phrases in a request that ask for this stack (`" in python"`), matched on the lower-case
    /// request padded with spaces.
    #[serde(default)]
    pub asked_by: Vec<String>,
    /// What the builder is told; `{editor}`, `{player}` and `{ext}` are filled in. `None` gets
    /// the generic instruction.
    #[serde(default)]
    pub steps: Option<String>,
    /// package.json dependencies that show a build uses this stack (`next`, `vite`); in a
    /// person's own project, Cargo.toml's too (`eframe`).
    #[serde(default)]
    pub uses: Vec<String>,
    /// What a person's own project must depend on as well to count as this stack (`react`).
    #[serde(default)]
    pub needs: Vec<String>,
    /// Files, relative to a folder, that show a person's own project is built with this stack
    /// (`project.godot`); a leading `*` matches by the name's ending (`*.uproject`).
    #[serde(default)]
    pub found_by: Vec<String>,
    /// Its build is a website: a project built with it and nothing to recreate is one.
    #[serde(default)]
    pub site: bool,
    /// The program its build can't run without (`cargo`), and how to get it.
    #[serde(default)]
    pub program: Option<String>,
    #[serde(default)]
    pub program_from: Option<String>,
    /// The `[[toolchain]]` row that builds it (`Rust`, `Python`, `Node.js`…), if one names it.
    #[serde(default)]
    pub toolchain: Option<String>,
}

pub use craft_anything_engine::toolchains::{Toolchain, built_in, found_in as toolchain_in};

/// The stack that leaves the choice to the builder (a website's default); [`used_in`] then
/// records the one it used.
pub const BUILDERS_CHOICE: &str = "Builder's choice";

/// A component or styling library asked for on top of a stack ("shadcn/ui", "Tailwind CSS").
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Library {
    pub name: String,
    /// Phrases in a request that ask for it, matched like [`Stack::asked_by`].
    #[serde(default)]
    pub asked_by: Vec<String>,
    /// The stacks it works with, by name; empty = any.
    #[serde(default)]
    pub stacks: Vec<String>,
    /// What the builder is told, after the stack's steps.
    pub steps: String,
}

#[derive(Debug, Default, serde::Deserialize)]
struct Table {
    #[serde(default)]
    stack: Vec<Stack>,
    #[serde(default)]
    library: Vec<Library>,
}

/// The built-in table. A table that doesn't parse (a broken edit) leaves it empty, so every
/// stack gets the generic instruction instead of the app failing; a test keeps it parsing.
fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| parse(include_str!("stacks.toml")).unwrap_or_default())
}

pub fn catalogue() -> &'static [Stack] {
    &table().stack
}

fn parse(text: &str) -> Result<Table, toml::de::Error> {
    toml::from_str::<Table>(text)
}

/// A request as `asked_by` phrases are matched: lower-case words, punctuation turned into
/// spaces, padded with spaces (`"Use Next.js, shadcn/ui"` → `" use next js shadcn ui "`).
pub fn words(request: &str) -> String {
    let lower = request.to_lowercase();
    format!(" {} ", lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" "))
}

pub fn get(name: &str) -> Option<&'static Stack> {
    catalogue().iter().find(|s| s.name == name)
}

/// The stack's own program (`cargo`) when `have` says this computer doesn't have it, and how to get it.
pub fn missing_program(stack: &str, have: impl Fn(&str) -> bool) -> Option<(&'static str, &'static str)> {
    let s = get(stack)?;
    let program = s.program.as_deref().filter(|p| !have(p))?;
    Some((program, s.program_from.as_deref().unwrap_or_default()))
}

/// A stack picked by name: its full name or its engine id, ignoring case (`unity`, `Godot`,
/// `React + TypeScript (Vite)`).
pub fn resolve(name: &str) -> Option<&'static Stack> {
    let n = name.trim();
    catalogue().iter().find(|s| s.name.eq_ignore_ascii_case(n) || s.engine.as_deref().is_some_and(|e| e.eq_ignore_ascii_case(n)))
}

/// The first stack whose phrases appear in `words` (a request through [`words`]).
pub fn asked_for(words: &str) -> Option<&'static Stack> {
    catalogue().iter().find(|s| s.asked_by.iter().any(|p| words.contains(p.as_str())))
}

/// The dependencies a folder's package.json and Cargo.toml name, for every rule that reads them.
fn dependencies(dir: &std::path::Path) -> Vec<String> {
    let mut deps = craft_anything_engine::outputs::npm_dependencies(dir);
    deps.extend(craft_anything_engine::outputs::cargo_dependencies(dir));
    deps
}

/// The stack a build in `app` uses, from its package.json dependencies (the first stack in table
/// order whose `uses` it lists); None when it has none or names no stack. Cargo.toml doesn't
/// replace a stack the request named; the toolchain rule covers a build that moved language.
pub fn used_in(app: &std::path::Path) -> Option<&'static Stack> {
    let deps = craft_anything_engine::outputs::npm_dependencies(app);
    catalogue().iter().find(|s| s.uses.iter().any(|d| deps.contains(d)))
}

/// The stack a folder of the person's own files is built with: an engine's project file says
/// most, then the dependencies, then the other files that name a stack. None when nothing does.
pub fn found_in(dir: &std::path::Path) -> Option<&'static Stack> {
    let by_file = |engine: bool| catalogue().iter().filter(|s| s.engine.is_some() == engine).find(|s| s.found_by.iter().any(|f| has_file(dir, f)));
    let deps = dependencies(dir);
    let by_deps = || catalogue().iter().find(|s| s.uses.iter().any(|d| deps.contains(d)) && s.needs.iter().all(|d| deps.contains(d)));
    by_file(true).or_else(by_deps).or_else(|| by_file(false))
}

fn has_file(dir: &std::path::Path, name: &str) -> bool {
    match name.strip_prefix('*') {
        Some(ending) => std::fs::read_dir(dir).into_iter().flatten().flatten().any(|e| e.file_name().to_string_lossy().ends_with(ending)),
        None => dir.join(name).is_file(),
    }
}

/// The toolchain that builds `stack`, when the table names one.
pub fn toolchain_for(stack: &str) -> Option<&'static Toolchain> {
    craft_anything_engine::toolchains::get(get(stack)?.toolchain.as_deref()?)
}

/// The stack to record for `toolchain` that was built in `app`: the first stack with that
/// toolchain whose declared dependencies or files are here, then the toolchain's own
/// `default_stack` from the table. None for a toolchain with neither (Go, .NET, Maven, Gradle,
/// Make): the recorded toolchain still tells the check. It searches every stack rather than taking
/// the first stack from either dependency file, so a stale package.json can't hide a matching
/// Cargo dependency.
pub fn stack_for_toolchain(toolchain: &str, app: &std::path::Path) -> Option<&'static Stack> {
    let deps = dependencies(app);
    catalogue()
        .iter()
        .find(|s| s.toolchain.as_deref() == Some(toolchain) && s.uses.iter().any(|d| deps.contains(d)) && s.needs.iter().all(|d| deps.contains(d)))
        .or_else(|| catalogue().iter().find(|s| s.toolchain.as_deref() == Some(toolchain) && s.found_by.iter().any(|f| has_file(app, f))))
        .or_else(|| craft_anything_engine::toolchains::get(toolchain).and_then(|t| t.default_stack.as_deref()).and_then(get))
}

/// The toolchain to build or test `app` with, in precedence order: the recorded build toolchain
/// when its files are here, the recorded stack's toolchain when its files are here, the first
/// source-backed toolchain, then the marker-only fallback for a project whose sources aren't
/// written yet. Shared by the build and own-tests checks so they agree.
pub fn toolchain_for_build(app: &std::path::Path, stack: &str, build_toolchain: Option<&str>) -> Option<&'static Toolchain> {
    // An incomplete scan can't prove the recorded toolchain is gone, so it keeps it, exactly as
    // record_after_build does. A completed scan with no sources falls through.
    if let Some(tool) = build_toolchain.and_then(toolchains::get) {
        match toolchains::built_in_named_scan(app, &tool.name) {
            SourceScan::Found | SourceScan::Unknown => return Some(tool),
            SourceScan::None => {}
        }
    }
    if let Some(tool) = toolchain_for(stack) {
        match toolchains::built_in_named_scan(app, &tool.name) {
            SourceScan::Found | SourceScan::Unknown => return Some(tool),
            SourceScan::None => {}
        }
    }
    built_in(app).or_else(|| toolchain_in(app))
}

/// Record the stack and toolchain a finished build actually used, by one rule: a non-game stack
/// whose dependencies name another stack gives way; a recorded toolchain whose markers and sources
/// are still here is kept, even when both it and another toolchain have source trees; only when
/// the recorded toolchain is absent or invalid does the stack's toolchain, then detection, choose
/// one. An incomplete source scan keeps the recorded choice instead of replacing it. Game engines
/// keep their stack; Rust's game default may change.
pub fn record_after_build(manifest: &mut Manifest, app: &std::path::Path) {
    let mut engine = game::Builder::from_stack(&manifest.stack);
    if engine.is_none()
        && let Some(s) = used_in(app)
    {
        manifest.stack = s.name.clone();
        engine = game::Builder::from_stack(&manifest.stack);
    }
    let toolchain_here = engine.is_none() || engine == Some(game::Builder::Rust);
    if !toolchain_here {
        manifest.build_toolchain = None;
        return;
    }
    // Keep what the last finished turn used while its files are still here. An incomplete scan
    // can't prove they're gone, so it keeps the recorded toolchain too.
    if let Some(tool) = manifest.build_toolchain.as_deref().and_then(toolchains::get) {
        match toolchains::built_in_named_scan(app, &tool.name) {
            SourceScan::Found | SourceScan::Unknown => return,
            SourceScan::None => manifest.build_toolchain = None,
        }
    }
    // The recorded toolchain is absent or invalid: use the recorded stack's toolchain when it is
    // here. An incomplete scan is not evidence that it isn't.
    if let Some(tool) = toolchain_for(&manifest.stack) {
        match toolchains::built_in_named_scan(app, &tool.name) {
            SourceScan::Found => {
                manifest.build_toolchain = Some(tool.name.clone());
                return;
            }
            SourceScan::Unknown => return,
            SourceScan::None => {}
        }
    }
    // Nothing recorded or named by the stack can be built: detect the first source-backed
    // toolchain and record the stack that matches it.
    if let Some(tool) = built_in(app) {
        manifest.build_toolchain = Some(tool.name.clone());
        if let Some(s) = stack_for_toolchain(&tool.name, app) {
            manifest.stack = s.name.clone();
        }
    } else {
        manifest.build_toolchain = None;
    }
}

/// The libraries `request` asks for that work with `stack`, in table order.
pub fn libraries_for(request: &str, stack: &str) -> Vec<&'static Library> {
    let w = words(request);
    table()
        .library
        .iter()
        .filter(|l| l.asked_by.iter().any(|p| w.contains(p.as_str())) && (l.stacks.is_empty() || l.stacks.iter().any(|s| s == stack)))
        .collect()
}

/// What the builder is told for `stack`. `editor`: the game engine's editor for engine stacks.
pub fn steps(stack: &str, editor: Option<&std::path::Path>) -> String {
    let Some(text) = get(stack).and_then(|s| s.steps.as_deref()) else {
        return format!("Use {stack}. Create the project in app/ and make sure it builds or runs without errors.");
    };
    let (player, ext) = if cfg!(windows) {
        ("StandaloneWindows64", "exe")
    } else if cfg!(target_os = "macos") {
        ("StandaloneOSX", "app")
    } else {
        ("StandaloneLinux64", "x86_64")
    };
    let editor = editor.map(|e| e.display().to_string()).unwrap_or_default();
    craft_anything_platform::text::fill(text, &[("editor", &editor), ("player", player), ("ext", ext)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_says_what_it_is_built_with() {
        // Each case in a folder of its own.
        let found = |files: &[(&str, &str)]| {
            let d = tempfile::tempdir().unwrap();
            for (name, text) in files {
                let path = d.path().join(name);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, text).unwrap();
            }
            found_in(d.path()).map(|s| s.name.as_str())
        };
        let vite = r#"{"dependencies":{"react":"19"},"devDependencies":{"vite":"7"}}"#;
        assert_eq!(found(&[("project.godot", "")]), Some("Godot"));
        assert_eq!(found(&[("ProjectSettings/ProjectVersion.txt", "")]), Some("Unity"));
        assert_eq!(found(&[("Game.uproject", "{}")]), Some("Unreal Engine"));
        assert_eq!(found(&[("project.godot", ""), ("package.json", vite)]), Some("Godot"), "an engine's file says most");
        assert_eq!(found(&[("package.json", vite), ("index.html", "")]), Some("React + TypeScript (Vite)"));
        assert_eq!(
            found(&[("package.json", r#"{"dependencies":{"vue":"3"},"devDependencies":{"vite":"7"}}"#)]),
            Some(BUILDERS_CHOICE),
            "Vite alone isn't React"
        );
        assert_eq!(found(&[("package.json", r#"{"dependencies":{"svelte":"5"}}"#), ("index.html", "")]), Some(BUILDERS_CHOICE));
        assert_eq!(found(&[("Cargo.toml", "[dependencies]\neframe = \"0.30\"\n")]), Some("Rust + egui (eframe)"));
        assert_eq!(found(&[("Cargo.toml", "[dependencies]\nmacroquad = \"0.4\"\n")]), Some("Rust + macroquad"));
        assert_eq!(found(&[("Cargo.toml", "[dependencies]\nmacroquad = \"0.4\"\negui = \"0.30\"\n")]), Some("Rust + macroquad"), "a game with an egui overlay");
        assert_eq!(found(&[("Cargo.toml", "[dependencies]\niced = \"0.13\"\n")]), Some(BUILDERS_CHOICE));
        assert_eq!(found(&[("Cargo.toml", "not [ toml")]), Some(BUILDERS_CHOICE));
        assert_eq!(found(&[("pyproject.toml", "")]), Some("Python"));
        assert_eq!(found(&[("requirements.txt", "")]), Some("Python"));
        assert_eq!(found(&[("go.mod", "")]), Some(BUILDERS_CHOICE));
        assert_eq!(found(&[("App.csproj", "")]), Some(BUILDERS_CHOICE));
        assert_eq!(found(&[("index.html", "")]), Some("Plain HTML and CSS"));
        assert_eq!(found(&[("notes.md", "")]), None);
    }

    /// Verify file-based toolchain detection and that every shared row has build/test commands.
    #[test]
    fn a_folder_says_how_it_builds_and_tests_itself() {
        let tool = |file: &str| {
            let d = tempfile::tempdir().unwrap();
            std::fs::write(d.path().join(file), "").unwrap();
            toolchain_in(d.path()).map(|t| t.name.as_str())
        };
        assert_eq!(tool("Cargo.toml"), Some("Rust"));
        assert_eq!(tool("package.json"), Some("Node.js"));
        assert_eq!(tool("requirements.txt"), Some("Python"));
        assert_eq!(tool("go.mod"), Some("Go"));
        assert_eq!(tool("App.csproj"), Some(".NET"));
        assert_eq!(tool("Shop.sln"), Some(".NET"));
        assert_eq!(tool("pom.xml"), Some("Maven"));
        assert_eq!(tool("build.gradle.kts"), Some("Gradle"));
        assert_eq!(tool("Makefile"), Some("Make"));
        assert_eq!(tool("notes.md"), None);
        for t in craft_anything_engine::toolchains::catalogue() {
            assert!(!t.found_by.is_empty() && !t.build.is_empty() && !t.test.is_empty(), "{}", t.name);
        }
    }

    #[test]
    fn the_table_parses_and_covers_every_engine() {
        let names: Vec<&str> = catalogue().iter().map(|s| s.name.as_str()).collect();
        assert!(names.len() >= 7, "{names:?}");
        for b in craft_anything_engines::game::Builder::ALL {
            assert!(get(b.stack()).is_some(), "no stack in stacks.toml for {}", b.stack());
        }
        for s in catalogue() {
            if let Some(t) = &s.toolchain {
                assert!(craft_anything_engine::toolchains::get(t).is_some(), "{} names unknown toolchain {t}", s.name);
            }
        }
        assert_eq!(get("Unity").and_then(|s| s.engine.as_deref()), Some("unity"));
        assert!(parse("[[stack]]\nname = 3").is_err(), "a broken table is an error, not a panic");
        for l in &table().library {
            assert!(!l.steps.is_empty() && !l.asked_by.is_empty(), "{l:?}");
            assert!(l.stacks.iter().all(|s| get(s).is_some()), "{} names a stack that isn't in the table", l.name);
        }
    }

    /// R2.9: a marker with no source of its own doesn't count as built; another language's
    /// sources do, so a stale Cargo.toml gives way to a Python entrypoint.
    #[test]
    fn a_folder_is_built_with_a_toolchain_only_when_its_sources_are_there() {
        let built = |files: &[(&str, &str)]| {
            let d = tempfile::tempdir().unwrap();
            for (name, text) in files {
                let p = d.path().join(name);
                if let Some(parent) = p.parent() {
                    std::fs::create_dir_all(parent).unwrap();
                }
                std::fs::write(p, text).unwrap();
            }
            built_in(d.path()).map(|t| t.name.as_str())
        };
        assert_eq!(built(&[("Cargo.toml", "")]), None, "a manifest without sources isn't built");
        assert_eq!(built(&[("Cargo.toml", ""), ("src/main.rs", "")]), Some("Rust"));
        assert_eq!(built(&[("Cargo.toml", ""), ("src/bin/cli.rs", "")]), Some("Rust"), "src/bin is Rust source, not output");
        assert_eq!(built(&[("Cargo.toml", ""), ("main.py", "")]), Some("Python"), "a stale Cargo.toml gives way to Python");
        assert_eq!(built(&[("package.json", ""), ("index.ts", "")]), Some("Node.js"));
        assert_eq!(built(&[("pyproject.toml", ""), ("app.py", "")]), Some("Python"));
        assert_eq!(built(&[("go.mod", ""), ("main.go", "")]), Some("Go"));
        assert_eq!(built(&[("App.csproj", ""), ("Program.cs", "")]), Some(".NET"));
        assert_eq!(built(&[("App.csproj", ""), ("bin/Generated.cs", "")]), None, ".NET build output isn't source");
        assert_eq!(built(&[("pom.xml", ""), ("src/Main.java", "")]), Some("Maven"));
        assert_eq!(built(&[("build.gradle", ""), ("src/main/groovy/App.groovy", "")]), Some("Gradle"), "Groovy is Gradle source");
        assert_eq!(
            built(&[("build.gradle", ""), ("main.py", ""), ("build/generated/Old.groovy", "")]),
            Some("Python"),
            "a Gradle build folder is output, not source"
        );
        assert_eq!(built(&[("build.gradle", ""), ("build/generated/Old.groovy", "")]), None, "Gradle output alone isn't a project");
        assert_eq!(built(&[("Makefile", "")]), Some("Make"));
        assert_eq!(built(&[("main.py", "")]), Some("Python"), "a root entrypoint alone");
        assert_eq!(built(&[("notes.md", "")]), None);

        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("Cargo.toml"), "").unwrap();
        assert!(!craft_anything_engine::toolchains::built_in_named(d.path(), "Rust"), "no sources, no build");
        std::fs::create_dir_all(d.path().join("src")).unwrap();
        std::fs::write(d.path().join("src/main.rs"), "").unwrap();
        assert!(craft_anything_engine::toolchains::built_in_named(d.path(), "Rust"));
    }

    #[test]
    fn stacks_name_the_toolchain_that_builds_them() {
        assert_eq!(toolchain_for("Python").map(|t| t.name.as_str()), Some("Python"));
        assert_eq!(toolchain_for("Rust + egui (eframe)").map(|t| t.name.as_str()), Some("Rust"));
        assert_eq!(toolchain_for("React + TypeScript (Vite)").map(|t| t.name.as_str()), Some("Node.js"));
        assert_eq!(toolchain_for("Unity"), None);
        for t in craft_anything_engine::toolchains::catalogue() {
            if let Some(stack) = &t.default_stack {
                assert!(get(stack).is_some(), "{} names unknown default stack {stack}", t.name);
            }
        }
    }

    /// A stack's `needs` are part of its identity: a package that lists `vite` without `react`
    /// isn't the React stack, so it records no stack rather than the wrong one.
    #[test]
    fn a_stack_that_needs_another_dependency_does_not_match_without_it() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("package.json"), r#"{"dependencies": {"vite": "6"}}"#).unwrap();
        assert_eq!(stack_for_toolchain("Node.js", d.path()), None, "vite without react isn't the React stack");
        std::fs::write(d.path().join("package.json"), r#"{"dependencies": {"vite": "6", "react": "19"}}"#).unwrap();
        assert_eq!(stack_for_toolchain("Node.js", d.path()).map(|s| s.name.as_str()), Some("React + TypeScript (Vite)"));
    }

    /// The toolchain is recorded even when it is the one the stack names: the check then uses the
    /// same toolchain the build used instead of deriving it again.
    #[test]
    fn a_finished_build_records_the_toolchain_even_when_the_stack_names_it() {
        let d = tempfile::tempdir().unwrap();
        let mut ws = craft_anything_engine::workspace::Workspace::create(d.path(), "P", "x", craft_anything_engine::workspace::TargetRef::None).unwrap();
        std::fs::create_dir_all(ws.app_dir()).unwrap();
        std::fs::write(ws.app_dir().join("main.py"), "").unwrap();
        ws.manifest.stack = "Python".into();
        let app = ws.app_dir();
        record_after_build(&mut ws.manifest, &app);
        assert_eq!(ws.manifest.build_toolchain.as_deref(), Some("Python"));
        assert_eq!(ws.manifest.stack, "Python");
    }

    /// A turn that changed nothing keeps the toolchain the last finished build used, even when
    /// the folder also has another language's complete source tree to detect.
    #[test]
    fn an_unchanged_turn_keeps_the_recorded_toolchain_when_both_source_trees_are_here() {
        let d = tempfile::tempdir().unwrap();
        let mut ws = craft_anything_engine::workspace::Workspace::create(d.path(), "P", "x", craft_anything_engine::workspace::TargetRef::None).unwrap();
        std::fs::create_dir_all(ws.app_dir().join("src")).unwrap();
        std::fs::write(ws.app_dir().join("main.py"), "").unwrap();
        std::fs::write(ws.app_dir().join("Cargo.toml"), "[dependencies]\neframe = \"0.30\"\n").unwrap();
        std::fs::write(ws.app_dir().join("src/main.rs"), "").unwrap();
        ws.manifest.stack = "Rust + egui (eframe)".into();
        ws.manifest.build_toolchain = Some("Python".into());
        let app = ws.app_dir();
        record_after_build(&mut ws.manifest, &app);
        assert_eq!(ws.manifest.build_toolchain.as_deref(), Some("Python"), "the recorded toolchain's files are still here");
        assert_eq!(ws.manifest.stack, "Rust + egui (eframe)", "an unchanged turn keeps the recorded stack");
    }

    /// The build selection and the recorder share one contract: an unknown scan keeps the
    /// recorded toolchain, even when another language's source tree is readable.
    #[cfg(unix)]
    #[test]
    fn build_selection_keeps_recorded_python_when_its_scan_is_unknown() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let mut ws = craft_anything_engine::workspace::Workspace::create(d.path(), "P", "x", craft_anything_engine::workspace::TargetRef::None).unwrap();
        let nested = ws.app_dir().join("pkg");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(ws.app_dir().join("pyproject.toml"), "").unwrap();
        std::fs::write(nested.join("main.py"), "").unwrap();
        std::fs::create_dir_all(ws.app_dir().join("src")).unwrap();
        std::fs::write(ws.app_dir().join("Cargo.toml"), "").unwrap();
        std::fs::write(ws.app_dir().join("src/main.rs"), "").unwrap();
        std::fs::set_permissions(&nested, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read_dir(&nested).is_ok() {
            std::fs::set_permissions(&nested, std::fs::Permissions::from_mode(0o755)).unwrap();
            return;
        }
        ws.manifest.stack = "Rust + egui (eframe)".into();
        ws.manifest.build_toolchain = Some("Python".into());
        let app = ws.app_dir();
        let selected = toolchain_for_build(&app, &ws.manifest.stack, ws.manifest.build_toolchain.as_deref());
        std::fs::set_permissions(&nested, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(selected.map(|t| t.name.as_str()), Some("Python"), "the recorded toolchain is kept on an unknown scan");
    }

    /// An unreadable folder in the recorded toolchain's source scan doesn't prove its sources are
    /// gone, so the recorded toolchain must not be replaced.
    #[cfg(unix)]
    #[test]
    fn an_unknown_source_scan_keeps_the_recorded_toolchain() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let mut ws = craft_anything_engine::workspace::Workspace::create(d.path(), "P", "x", craft_anything_engine::workspace::TargetRef::None).unwrap();
        let nested = ws.app_dir().join("pkg");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(ws.app_dir().join("pyproject.toml"), "").unwrap();
        std::fs::write(nested.join("main.py"), "").unwrap();
        std::fs::set_permissions(&nested, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read_dir(&nested).is_ok() {
            // A test run as root can still list it; there is no unknown scan to exercise.
            std::fs::set_permissions(&nested, std::fs::Permissions::from_mode(0o755)).unwrap();
            return;
        }
        ws.manifest.stack = "Rust + egui (eframe)".into();
        ws.manifest.build_toolchain = Some("Python".into());
        let app = ws.app_dir();
        record_after_build(&mut ws.manifest, &app);
        std::fs::set_permissions(&nested, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(ws.manifest.build_toolchain.as_deref(), Some("Python"));
        assert_eq!(ws.manifest.stack, "Rust + egui (eframe)");
    }

    #[test]
    fn a_recorded_toolchain_names_the_stack_to_show() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("main.py"), "").unwrap();
        assert_eq!(stack_for_toolchain("Python", d.path()).map(|s| s.name.as_str()), Some("Python"));
        std::fs::write(d.path().join("Cargo.toml"), "[dependencies]\neframe = \"0.30\"\n").unwrap();
        std::fs::create_dir_all(d.path().join("src")).unwrap();
        std::fs::write(d.path().join("src/main.rs"), "").unwrap();
        assert_eq!(stack_for_toolchain("Rust", d.path()).map(|s| s.name.as_str()), Some("Rust + egui (eframe)"), "the stack its Cargo dependencies name");
        assert_eq!(stack_for_toolchain("Go", d.path()), None, "Go has no stack row");
    }

    /// R2.9 review: a stack named by one dependency file must not stop the search before another
    /// file names the matching stack for the toolchain that was built.
    #[test]
    fn a_matching_stack_is_found_after_another_stack_from_used_in() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("package.json"), r#"{"dependencies": {"next": "16", "react": "19"}}"#).unwrap();
        std::fs::write(d.path().join("Cargo.toml"), "[dependencies]\neframe = \"0.37\"\n").unwrap();
        assert_eq!(used_in(d.path()).map(|s| s.name.as_str()), Some("Next.js + TypeScript"), "the stale package.json names a stack");
        assert_eq!(stack_for_toolchain("Node.js", d.path()).map(|s| s.name.as_str()), Some("Next.js + TypeScript"));
        assert_eq!(stack_for_toolchain("Rust", d.path()).map(|s| s.name.as_str()), Some("Rust + egui (eframe)"), "the Cargo stack is still found");
    }

    #[test]
    fn the_toolchain_choice_follows_the_recorded_one_when_its_files_are_here() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("Cargo.toml"), "[dependencies]\neframe = \"0.37\"\n").unwrap();
        std::fs::create_dir_all(d.path().join("src")).unwrap();
        std::fs::write(d.path().join("src/main.rs"), "").unwrap();
        assert_eq!(toolchain_for_build(d.path(), "Rust + egui (eframe)", Some("Rust")).map(|t| t.name.as_str()), Some("Rust"));
        assert_eq!(
            toolchain_for_build(d.path(), "Rust + egui (eframe)", Some("Python")).map(|t| t.name.as_str()),
            Some("Rust"),
            "an invalid recorded toolchain gives way to the stack's"
        );
        let both = tempfile::tempdir().unwrap();
        std::fs::write(both.path().join("Cargo.toml"), "[dependencies]\neframe = \"0.37\"\n").unwrap();
        std::fs::create_dir_all(both.path().join("src")).unwrap();
        std::fs::write(both.path().join("src/main.rs"), "").unwrap();
        std::fs::write(both.path().join("main.py"), "").unwrap();
        assert_eq!(
            toolchain_for_build(both.path(), "Rust + egui (eframe)", Some("Python")).map(|t| t.name.as_str()),
            Some("Python"),
            "a valid recorded toolchain wins even when the stack's is also valid"
        );
        let python = tempfile::tempdir().unwrap();
        std::fs::write(python.path().join("main.py"), "").unwrap();
        assert_eq!(toolchain_for_build(python.path(), "Rust + egui (eframe)", None).map(|t| t.name.as_str()), Some("Python"));
        let marker = tempfile::tempdir().unwrap();
        std::fs::write(marker.path().join("Cargo.toml"), "[package]\nname = \"new\"\n").unwrap();
        assert_eq!(
            toolchain_for_build(marker.path(), "Rust + egui (eframe)", None).map(|t| t.name.as_str()),
            Some("Rust"),
            "a project whose sources aren't written yet keeps the marker-only fallback"
        );
    }

    #[test]
    fn a_build_left_to_the_builder_is_recorded_under_the_stack_it_used() {
        let d = tempfile::tempdir().unwrap();
        assert!(
            get(BUILDERS_CHOICE)
                .is_some_and(|s| s.steps.as_deref().is_some_and(|t| t.starts_with("Use what their request names; otherwise choose the stack yourself")))
        );
        assert!(used_in(d.path()).is_none(), "no package.json");
        std::fs::write(d.path().join("Cargo.toml"), "[dependencies]\neframe = \"0.30\"\n").unwrap();
        assert!(used_in(d.path()).is_none(), "Cargo.toml never replaces a stack the request named");
        std::fs::write(d.path().join("package.json"), "not json").unwrap();
        assert!(used_in(d.path()).is_none(), "a broken package.json is no stack, not a crash");
        std::fs::write(d.path().join("package.json"), r#"{"dependencies": {"next": "16", "react": "19"}, "devDependencies": {"vite": "7"}}"#).unwrap();
        assert_eq!(used_in(d.path()).map(|s| s.name.as_str()), Some("Next.js + TypeScript"), "table order decides");
        std::fs::write(d.path().join("package.json"), r#"{"devDependencies": {"vite": "7"}}"#).unwrap();
        assert_eq!(used_in(d.path()).map(|s| s.name.as_str()), Some("React + TypeScript (Vite)"));
        std::fs::write(d.path().join("package.json"), r#"{"dependencies": {"astro": "5"}}"#).unwrap();
        assert!(used_in(d.path()).is_none(), "a stack the table doesn't know stays the builder's choice");
        assert_eq!(libraries_for("use shadcn", BUILDERS_CHOICE).len(), 1, "a library works with the builder's choice too");
    }

    #[test]
    fn spelled_variants_of_a_stack_and_its_libraries() {
        let next = Some("Next.js + TypeScript");
        for r in ["use nextjs", "Use Next.js", "built with next js", "in Next.JS please", "(Next.js) with React", "use next and shadcn", "a site in next"] {
            assert_eq!(asked_for(&words(r)).map(|s| s.name.as_str()), next, "{r}");
        }
        assert_eq!(asked_for(&words("what to do next")), None, "\"next\" alone isn't a stack");
        assert_eq!(asked_for(&words("build it with Vite")).map(|s| s.name.as_str()), Some("React + TypeScript (Vite)"));
        let names = |r: &str, stack: &str| libraries_for(r, stack).iter().map(|l| l.name.as_str()).collect::<Vec<_>>();
        for r in ["use shadcn", "with shadcn/ui", "shadcn-ui components", "ShadCN"] {
            assert_eq!(names(r, "Next.js + TypeScript"), ["shadcn/ui"], "{r}");
        }
        assert_eq!(names("shadcn and tailwind", "React + TypeScript (Vite)"), ["shadcn/ui", "Tailwind CSS"]);
        assert_eq!(names("TailwindCSS", "Next.js + TypeScript"), ["Tailwind CSS"]);
        assert!(names("shadcn", "Python").is_empty(), "a library only joins the stacks it works with");
        assert!(names("a plain page", "Next.js + TypeScript").is_empty());
    }

    #[test]
    fn named_game_frameworks_do_not_select_an_editor() {
        for (request, name) in [("Make a game in Phaser with Vite", "Phaser"), ("Make a game using Bevy", "Bevy")] {
            let stack = asked_for(&words(request)).unwrap();
            assert_eq!(stack.name, name);
            assert!(stack.engine.is_none());
            assert_eq!(resolve(name), Some(stack));
        }
        assert!(asked_for(&words("a game with a phaser cannon")).is_none());
    }

    #[test]
    fn named_stacks_and_placeholders() {
        assert_eq!(asked_for(&words("A tool in Python.")).map(|s| s.name.as_str()), Some("Python"));
        assert_eq!(asked_for(&words("a page with react")).map(|s| s.name.as_str()), Some("React + TypeScript (Vite)"));
        assert_eq!(asked_for(&words("a notes app")), None);
        assert_eq!(resolve("godot").map(|s| s.name.as_str()), Some("Godot"));
        assert_eq!(resolve(" unreal ").map(|s| s.name.as_str()), Some("Unreal Engine"));
        assert_eq!(resolve("rust + EGUI (eframe)").map(|s| s.name.as_str()), Some("Rust + egui (eframe)"));
        assert!(resolve("cobol").is_none());
        let unity = steps("Unity", Some(std::path::Path::new("/ed/Unity")));
        assert!(unity.contains("the editor is /ed/Unity") && !unity.contains('{'), "{unity}");
        assert_eq!(steps("Elm", None), "Use Elm. Create the project in app/ and make sure it builds or runs without errors.");
    }

    /// An editor path is inserted as it is, even when a folder in it is named like a later
    /// placeholder (`{player}`, `{ext}`), which chained replacements would fill in turn.
    #[test]
    fn an_editor_path_stays_literal() {
        for path in ["/ed/{ext}/Unity", "/ed/{player}/Unity", "/ed/{player}{ext}/Un{ity"] {
            let unity = steps("Unity", Some(std::path::Path::new(path)));
            assert!(unity.contains(&format!("the editor is {path})")), "{unity}");
            assert!(unity.contains(&format!("`\"{path}\" -batchmode")), "{unity}");
            assert!(!unity.contains("{player} player") && !unity.contains("Game.{ext}"), "{unity}");
        }
    }
}
