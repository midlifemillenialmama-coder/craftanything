//! Synthetic local captures only. The model records the real first builder turn, then stops.
use super::*;
use crate::agent::{ApiConversation, EngineHost, LoopConfig, ModelSlot};
use crate::model::{ChatModel, Delta, Stop, TurnRequest, TurnResult};
use craft_anything_engine::tools::web_common::{Image, encode_png};

struct BoundaryModel(Arc<Mutex<Vec<String>>>, CancelToken, PathBuf);
impl ChatModel for BoundaryModel {
    fn turn(&mut self, req: &TurnRequest<'_>, _: &mut dyn FnMut(Delta), _: &Stop) -> Result<TurnResult, ToolError> {
        let prompt: String = req.messages.iter().map(crate::model::Msg::text).collect();
        let route = self.2.join("routes/account/README.md");
        let private_route = if route.is_file() { std::fs::read_to_string(route).unwrap() } else { String::new() };
        let deep = ["how-it-works.md", "original.har"].into_iter().filter_map(|f| std::fs::read_to_string(self.2.join(f)).ok()).collect::<String>();
        self.0.lock().unwrap().push(format!("{prompt}{private_route}{deep}\n{}", std::fs::read_to_string(self.2.join("README.md")).unwrap()));
        self.1.cancel();
        Err(ToolError::new(ErrorCode::Cancelled, "Synthetic builder stopped at the boundary"))
    }
    fn model(&self) -> String {
        "boundary".into()
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    ws: Workspace,
    worker: Worker,
    seen: Arc<Mutex<Vec<String>>>,
}
impl Fixture {
    fn new(signed_in: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut ws =
            Workspace::create(dir.path(), "Synthetic site", "Rebuild this website", TargetRef::Website { url: "https://example.test/app".into() }).unwrap();
        ws.manifest.stack = "Synthetic stack".into();
        ws.manifest.capture_deep = Some(false);
        ws.save().unwrap();
        std::fs::create_dir_all(ws.capture_dir()).unwrap();
        let png = encode_png(&Image { width: 2, height: 2, rgba: vec![80; 16] }).unwrap();
        std::fs::write(ws.capture_dir().join("screenshot-1440.png"), png).unwrap();
        std::fs::write(ws.capture_dir().join("README.md"), "Synthetic account dashboard").unwrap();
        let metadata = json!({"url":"https://example.test/app", "signed_in":signed_in, "signed_in_site":"site-example.test_443", "sign_in_revision":"fixture-account-1", "sign_in_page":false,
            "screenshots":[{"file":"screenshot-1440.png"}]});
        std::fs::write(ws.capture_dir().join("capture.json"), serde_json::to_vec(&metadata).unwrap()).unwrap();
        let worker = check_worker(&ws, false);
        let profile = craft_anything_engine::tools::site_sign_in::sign_ins_dir(&worker.engine.session()).join("site-example.test_443");
        if signed_in {
            std::fs::create_dir_all(profile.join("profile")).unwrap();
            std::fs::write(profile.join("sign-in.json"), br#"{"version":1,"site":"site-example.test_443","saved_at":1,"revision":"fixture-account-1"}"#)
                .unwrap();
        }
        Self { _dir: dir, ws, worker, seen: Arc::default() }
    }
    fn metadata(&self, f: impl FnOnce(&mut Value)) {
        let path = self.ws.capture_dir().join("capture.json");
        let mut v = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        f(&mut v);
        std::fs::write(path, serde_json::to_vec(&v).unwrap()).unwrap();
    }
    fn run(&mut self, on_ask: impl Fn(&Inbox, &Request) + Send + Sync + 'static) -> ToolError {
        let inbox = self.worker.inbox.clone();
        self.worker.wake = Wake::new(move || {
            for r in inbox.pending() {
                on_ask(&inbox, &r);
            }
        });
        let model = BoundaryModel(self.seen.clone(), self.worker.stop.clone(), self.ws.capture_dir());
        let slot = ModelSlot {
            name: "Synthetic".into(),
            model: Box::new(model),
            local: true,
            effort: None,
            max_turns: 1,
            context_tokens: 0,
            tools: Default::default(),
        };
        let api = ApiConversation::start(Arc::new(EngineHost(self.worker.engine.clone())), vec![slot], LoopConfig::default()).unwrap();
        let entry = crate::BackendEntry {
            config: crate::BackendConfig {
                name: "Synthetic".into(),
                kind: BackendKind::OpenaiCompatible,
                model: "boundary".into(),
                base_url: None,
                ssh_host: None,
                enabled: true,
            },
            options: Default::default(),
        };
        let opts = RunOptions { message: Some("Continue with this capture".into()), max_rounds: 0, ..Default::default() };
        self.worker.run(&mut self.ws, entry, Some(api), None, KeptHistory::default(), &opts).unwrap_err()
    }
}

#[test]
fn rejected_signed_in_capture_never_reaches_the_builder() {
    let mut f = Fixture::new(true);
    f.run(|i, r| i.answer(&r.id, &Answer { allow: Some(false), ..Default::default() }).unwrap());
    assert!(f.seen.lock().unwrap().is_empty(), "rejected account content reached the first builder turn");
}

#[test]
fn signed_in_builder_waits_for_explicit_confirmation() {
    let mut f = Fixture::new(true);
    let engine = f.worker.engine.clone();
    let root = f.ws.root.clone();
    let seen = f.seen.clone();
    let asked = Arc::new(AtomicBool::new(false));
    let asked2 = asked.clone();
    f.run(move |i, r| {
        assert!(seen.lock().unwrap().is_empty(), "the builder ran before the inbox approval");
        asked2.store(true, Ordering::SeqCst);
        let shown = engine.call("project_show", json!({"project":root}), CallOptions::new(Caller::Cli)).unwrap().to_json();
        assert!(shown["result"]["questions"].as_array().unwrap().iter().any(|q| q["id"] == r.id && q["capture_review"].is_object()));
        engine.call("project_inbox_answer", json!({"project":root,"id":r.id,"allow":true}), CallOptions::new(Caller::Cli)).unwrap();
        assert!(i.pending().is_empty());
    });
    assert!(asked.load(Ordering::SeqCst), "signed-in study bypassed the inbox");
    assert_eq!(f.seen.lock().unwrap().len(), 1, "an approved capture reaches the builder");
    assert!(f.seen.lock().unwrap().first().unwrap().ends_with("Synthetic account dashboard"));
}

#[test]
fn unsigned_capture_preserves_the_existing_builder_workflow() {
    let mut f = Fixture::new(false);
    f.run(|_, _| panic!("unsigned capture unexpectedly asked for sign-in confirmation"));
    assert_eq!(f.seen.lock().unwrap().len(), 1);
}

#[test]
fn cancellation_or_an_empty_answer_cannot_release_account_content() {
    for cancel in [true, false] {
        let mut f = Fixture::new(true);
        let stop = f.worker.stop.clone();
        f.run(move |i, r| {
            if cancel {
                stop.cancel();
            } else {
                i.answer(&r.id, &Answer::default()).unwrap();
            }
        });
        assert!(f.seen.lock().unwrap().is_empty());
        assert!(f.worker.inbox.pending().is_empty(), "the stopped request must be withdrawn");
    }
}

#[test]
fn unreadable_or_missing_first_screenshots_stop_before_the_builder() {
    for missing in [true, false] {
        let mut f = Fixture::new(true);
        let file = f.ws.capture_dir().join("screenshot-1440.png");
        if missing {
            std::fs::remove_file(file).unwrap();
        } else {
            std::fs::write(file, "not a PNG").unwrap();
        }
        let error = f.run(|_, _| panic!("invalid pictures must not be offered for approval"));
        assert!(f.seen.lock().unwrap().is_empty());
        assert!(error.remediation.is_some(), "a broken preview needs recovery instructions");
    }
}

#[test]
fn an_expired_sign_in_is_never_offered_as_authenticated_content() {
    let mut f = Fixture::new(true);
    f.metadata(|v| v["sign_in_page"] = json!(true));
    let error = f.run(|_, _| panic!("a password page is not a successful signed-in study"));
    assert!(f.seen.lock().unwrap().is_empty());
    assert!(error.remediation.as_deref().unwrap_or_default().contains("sign"));
}

#[test]
fn changed_capture_or_target_is_not_authorized_by_the_waiting_answer() {
    for target in [true, false] {
        let mut f = Fixture::new(true);
        let root = f.ws.root.clone();
        f.run(move |i, r| {
            let mut ws = Workspace::open(&root).unwrap();
            if target {
                ws.manifest.target = TargetRef::Website { url: "https://other.test/app".into() };
                ws.save().unwrap();
            } else {
                std::fs::write(ws.capture_dir().join("README.md"), "Different account dashboard").unwrap();
            }
            i.answer(&r.id, &Answer { allow: Some(true), ..Default::default() }).unwrap();
        });
        assert!(f.seen.lock().unwrap().is_empty());
    }
}

#[test]
fn changing_the_saved_account_invalidates_a_waiting_confirmation() {
    let mut f = Fixture::new(true);
    let record = craft_anything_engine::tools::site_sign_in::sign_ins_dir(&f.worker.engine.session()).join("site-example.test_443/sign-in.json");
    f.run(move |i, r| {
        std::fs::write(&record, br#"{"version":1,"site":"site-example.test_443","saved_at":2,"revision":"fixture-account-2"}"#).unwrap();
        i.answer(&r.id, &Answer { allow: Some(true), ..Default::default() }).unwrap();
    });
    assert!(f.seen.lock().unwrap().is_empty());
}

#[test]
fn a_resumed_run_does_not_reuse_a_previous_confirmation() {
    let mut f = Fixture::new(true);
    f.run(|i, r| i.answer(&r.id, &Answer { allow: Some(true), ..Default::default() }).unwrap());
    let previous = f.seen.lock().unwrap().len();
    f.worker.stop = CancelToken::default();
    f.worker.seen.clear();
    let asked = Arc::new(AtomicBool::new(false));
    let asked2 = asked.clone();
    f.run(move |i, r| {
        asked2.store(true, Ordering::SeqCst);
        i.answer(&r.id, &Answer { allow: Some(false), ..Default::default() }).unwrap();
    });
    assert!(asked.load(Ordering::SeqCst), "resume must request fresh consent");
    assert_eq!(f.seen.lock().unwrap().len(), previous);
}

#[test]
fn an_unavailable_inbox_stops_before_the_builder() {
    let mut f = Fixture::new(true);
    std::fs::write(f.ws.inbox_dir(), "not a directory").unwrap();
    f.run(|_, _| panic!("the unavailable inbox cannot post an approval"));
    assert!(f.seen.lock().unwrap().is_empty());
}

#[test]
fn screenshot_paths_cannot_escape_project_storage() {
    for file in ["../outside.png", "/outside.png", "screenshot-../outside.png", "screenshot-https:outside.png"] {
        let mut f = Fixture::new(true);
        f.metadata(|v| v["screenshots"][0]["file"] = json!(file));
        f.run(|_, _| panic!("an invalid path must not be previewed"));
        assert!(f.seen.lock().unwrap().is_empty());
    }
}

#[cfg(unix)]
#[test]
fn screenshot_symlinks_cannot_escape_project_storage() {
    use std::os::unix::fs::symlink;
    let mut f = Fixture::new(true);
    let file = f.ws.capture_dir().join("screenshot-1440.png");
    let outside = f.ws.root.join("outside.png");
    std::fs::rename(&file, &outside).unwrap();
    symlink(outside, file).unwrap();
    f.run(|_, _| panic!("a symlink must not be previewed"));
    assert!(f.seen.lock().unwrap().is_empty());
}

#[test]
fn restart_replaces_abandoned_capture_requests_without_reusing_their_answers() {
    use craft_anything_engine::capture_review::CaptureReview;
    let mut f = Fixture::new(true);
    let old = f
        .worker
        .inbox
        .post_capture_review(CaptureReview { website: "https://old.test".into(), capture_sha256: "old-capture".into(), screenshots: Vec::new() })
        .unwrap();
    let old2 = old.clone();
    f.run(move |i, r| {
        assert_ne!(r.id, old2, "the abandoned approval must not be presented again");
        assert_eq!(i.pending().len(), 1);
        i.answer(&old2, &Answer { allow: Some(true), ..Default::default() }).unwrap();
        i.answer(&r.id, &Answer { allow: Some(false), ..Default::default() }).unwrap();
    });
    assert!(f.seen.lock().unwrap().is_empty(), "the old answer cannot release the new capture");
    assert!(!f.worker.inbox.contains_request(&old));
}

#[test]
fn missing_authentication_metadata_cannot_turn_a_private_capture_into_an_unsigned_one() {
    for missing in [true, false] {
        let mut f = Fixture::new(true);
        std::fs::remove_dir_all(craft_anything_engine::tools::site_sign_in::sign_ins_dir(&f.worker.engine.session())).unwrap();
        if missing {
            std::fs::remove_file(f.ws.capture_dir().join("capture.json")).unwrap();
        } else {
            f.metadata(|v| {
                v.as_object_mut().unwrap().remove("signed_in");
            });
        }
        f.run(|_, _| panic!("unknown authentication state cannot be offered as a valid capture"));
        assert!(f.seen.lock().unwrap().is_empty());
    }
}

// Review #6004691567: original synthetic fixture extended with local route captures only.
impl Fixture {
    fn save_account(&self, revision: &str) {
        let dir = craft_anything_engine::tools::site_sign_in::sign_ins_dir(&self.worker.engine.session()).join("site-example.test_443");
        std::fs::create_dir_all(dir.join("profile")).unwrap();
        std::fs::write(dir.join("sign-in.json"), serde_json::to_vec(&json!({"version":1,"site":"site-example.test_443","revision":revision})).unwrap())
            .unwrap();
    }

    fn route(&self, name: &str, signed_in: bool, revision: &str) {
        let dir = self.ws.capture_dir().join("routes").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let png = encode_png(&Image { width: 2, height: 2, rgba: vec![120; 16] }).unwrap();
        std::fs::write(dir.join("screenshot-1440.png"), png).unwrap();
        std::fs::write(dir.join("README.md"), if signed_in { "Synthetic private route content" } else { "Synthetic public route content" }).unwrap();
        let record = json!({"url":format!("https://example.test/{name}"),"signed_in":signed_in,"signed_in_site":"site-example.test_443",
            "sign_in_revision":revision,"sign_in_page":false,"screenshots":[{"file":"screenshot-1440.png"}]});
        std::fs::write(dir.join("capture.json"), serde_json::to_vec(&record).unwrap()).unwrap();
        let index = self.ws.capture_dir().join("routes.json");
        let mut routes: Vec<Value> = std::fs::read(&index).ok().map(|b| serde_json::from_slice(&b).unwrap()).unwrap_or_default();
        routes.push(json!({"url":format!("https://example.test/{name}"),"path":format!("/{name}"),"slug":name}));
        std::fs::write(index, serde_json::to_vec(&routes).unwrap()).unwrap();
    }

    /// A retry goes through the real worker's Study branch, stopping at the unavailable test
    /// browser. testing::session finds no installed engines, so this never opens a browser/network.
    fn assert_invalidated_and_restudied(&mut self) {
        let saved = Workspace::open(&self.ws.root).unwrap();
        assert_eq!(saved.manifest.capture_deep, None, "invalid evidence remained reusable on disk");
        self.ws = saved;
        self.worker.stop = CancelToken::default();
        self.worker.seen.clear();
        let (tx, rx) = std::sync::mpsc::channel();
        self.worker.tx = tx;
        let error = self.run(|_, _| panic!("retry reused the invalid capture instead of studying"));
        assert_eq!(error.code, ErrorCode::EngineMissing, "retry should reach the fresh study's browser preflight: {error:?}");
        assert!(rx.try_iter().any(|e| matches!(e, RunEvent::Agent(AgentEvent::ToolStarted { tool, .. }) if tool == "web_site_open")));
        assert!(self.seen.lock().unwrap().is_empty());
    }
}

#[test]
fn review_fix_initial_validation_failure_invalidates_and_retries_the_study() {
    for failure in ["missing", "invalid", "account"] {
        let mut f = Fixture::new(true);
        std::fs::create_dir_all(f.ws.app_dir()).unwrap();
        std::fs::write(f.ws.app_dir().join("keep.txt"), "unrelated work").unwrap();
        let shot = f.ws.capture_dir().join("screenshot-1440.png");
        match failure {
            "missing" => std::fs::remove_file(shot).unwrap(),
            "invalid" => std::fs::write(shot, "damaged PNG").unwrap(),
            _ => f.save_account("replacement-account"),
        }
        f.run(|_, _| panic!("invalid capture must not ask for approval"));
        assert!(f.seen.lock().unwrap().is_empty());
        f.assert_invalidated_and_restudied();
        assert_eq!(std::fs::read_to_string(f.ws.app_dir().join("keep.txt")).unwrap(), "unrelated work");
    }
}

#[test]
fn review_fix_post_approval_failure_invalidates_and_retries_the_study() {
    for failure in ["account", "bytes", "missing", "invalid"] {
        let mut f = Fixture::new(true);
        let root = f.ws.root.clone();
        let config = craft_anything_engine::tools::site_sign_in::sign_ins_dir(&f.worker.engine.session());
        f.run(move |i, r| {
            match failure {
                "account" => std::fs::write(
                    config.join("site-example.test_443/sign-in.json"),
                    br#"{"version":1,"site":"site-example.test_443","revision":"replacement"}"#,
                )
                .unwrap(),
                "bytes" => std::fs::write(root.join("capture/README.md"), "changed capture").unwrap(),
                "missing" => std::fs::remove_file(root.join("capture/screenshot-1440.png")).unwrap(),
                _ => std::fs::write(root.join("capture/screenshot-1440.png"), "damaged PNG").unwrap(),
            }
            i.answer(&r.id, &Answer { allow: Some(true), ..Default::default() }).unwrap();
        });
        assert!(f.seen.lock().unwrap().is_empty());
        f.assert_invalidated_and_restudied();
    }
}

#[test]
fn review_fix_unchanged_approval_and_cancellation_preserve_usable_captures() {
    for cancel in [false, true] {
        let mut f = Fixture::new(true);
        let stop = f.worker.stop.clone();
        let original = std::fs::read(f.ws.capture_dir().join("capture.json")).unwrap();
        f.run(move |i, r| {
            if cancel {
                stop.cancel();
            } else {
                i.answer(&r.id, &Answer { allow: Some(true), ..Default::default() }).unwrap();
            }
        });
        assert_eq!(Workspace::open(&f.ws.root).unwrap().manifest.capture_deep, Some(false));
        assert_eq!(std::fs::read(f.ws.capture_dir().join("capture.json")).unwrap(), original);
        assert_eq!(f.seen.lock().unwrap().len(), usize::from(!cancel));
    }
}

#[test]
fn review_fix_unsigned_root_with_private_route_waits_for_the_right_screenshots() {
    for allow in [false, true] {
        let mut f = Fixture::new(false);
        f.save_account("fixture-account-1");
        f.route("account", true, "fixture-account-1");
        let root = f.ws.root.clone();
        let seen = f.seen.clone();
        let asked = Arc::new(AtomicBool::new(false));
        let asked2 = asked.clone();
        f.run(move |i, r| {
            assert!(seen.lock().unwrap().is_empty(), "private route reached the builder before confirmation");
            let review = r.capture_review.as_ref().unwrap();
            assert!(!review.screenshots.is_empty());
            assert!(
                review.screenshots.iter().all(|s| s.file.starts_with("routes/account/")),
                "unsigned root screenshots cannot stand in for private-route review"
            );
            for shot in &review.screenshots {
                assert!(review.preview_bytes(&root, shot).is_ok());
            }
            asked2.store(true, Ordering::SeqCst);
            i.answer(&r.id, &Answer { allow: Some(allow), ..Default::default() }).unwrap();
        });
        assert!(asked.load(Ordering::SeqCst), "private route bypassed confirmation");
        assert_eq!(f.seen.lock().unwrap().len(), usize::from(allow));
        if allow {
            assert!(f.seen.lock().unwrap()[0].contains("Synthetic private route content"));
        }
    }
}

#[test]
fn review_fix_mixed_route_accounts_fail_closed() {
    let mut f = Fixture::new(true);
    f.route("account", true, "another-account");
    f.run(|_, _| panic!("mixed accounts cannot be authorized by a root-page preview"));
    assert!(f.seen.lock().unwrap().is_empty());
    f.assert_invalidated_and_restudied();
}

#[test]
fn review_fix_consistent_signed_in_pages_are_reviewed_together() {
    let mut f = Fixture::new(true);
    f.route("account", true, "fixture-account-1");
    f.route("settings", true, "fixture-account-1");
    let root = f.ws.root.clone();
    let seen = f.seen.clone();
    f.run(move |i, r| {
        assert!(seen.lock().unwrap().is_empty());
        let review = r.capture_review.as_ref().unwrap();
        for dir in ["screenshot-1440.png", "routes/account/screenshot-1440.png", "routes/settings/screenshot-1440.png"] {
            let shot = review.screenshots.iter().find(|s| s.file == dir).expect("every signed-in page needs its preview");
            assert!(review.preview_bytes(&root, shot).is_ok());
        }
        i.answer(&r.id, &Answer { allow: Some(true), ..Default::default() }).unwrap();
    });
    assert_eq!(f.seen.lock().unwrap().len(), 1);
    assert_eq!(Workspace::open(&f.ws.root).unwrap().manifest.capture_deep, Some(false));
}

#[test]
fn review_fix_fully_unsigned_routes_keep_the_unsigned_workflow() {
    let mut f = Fixture::new(false);
    f.route("account", false, "unused");
    // Public redirects do not establish or require a saved-account identity.
    f.metadata(|v| v["url"] = json!("https://public.example.test/landing"));
    f.run(|_, _| panic!("fully unsigned study must not require private-content approval"));
    assert_eq!(f.seen.lock().unwrap().len(), 1);
}

#[test]
fn review_fix_ambiguous_nested_metadata_fails_closed() {
    for failure in ["missing", "invalid", "unknown", "unsafe-index", "missing-route", "unsafe-shot", "foreign-site"] {
        let mut f = Fixture::new(true);
        f.route("account", true, "fixture-account-1");
        let record = f.ws.capture_dir().join("routes/account/capture.json");
        match failure {
            "missing" => std::fs::remove_file(record).unwrap(),
            "invalid" => std::fs::write(record, "not JSON").unwrap(),
            "unsafe-index" => std::fs::write(f.ws.capture_dir().join("routes.json"), br#"[{"slug":"../outside"}]"#).unwrap(),
            "missing-route" => {
                std::fs::write(f.ws.capture_dir().join("routes.json"), br#"[{"slug":"not-captured","url":"https://example.test/not-captured"}]"#).unwrap()
            }
            _ => {
                let mut v: Value = serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
                match failure {
                    "unknown" => {
                        v.as_object_mut().unwrap().remove("signed_in");
                    }
                    "unsafe-shot" => v["screenshots"][0]["file"] = json!("../screenshot-1440.png"),
                    _ => v["signed_in_site"] = json!("site-other.test_443"),
                }
                std::fs::write(record, serde_json::to_vec(&v).unwrap()).unwrap();
            }
        }
        f.run(|_, _| panic!("ambiguous nested content must not be offered for approval"));
        assert!(f.seen.lock().unwrap().is_empty());
        f.assert_invalidated_and_restudied();
    }
}

#[test]
fn review_fix_changed_private_route_while_pending_cannot_reach_the_builder() {
    let mut f = Fixture::new(false);
    f.save_account("fixture-account-1");
    f.route("account", true, "fixture-account-1");
    let route = f.ws.capture_dir().join("routes/account/README.md");
    let asked = Arc::new(AtomicBool::new(false));
    let asked2 = asked.clone();
    f.run(move |i, r| {
        asked2.store(true, Ordering::SeqCst);
        std::fs::write(&route, "different private route").unwrap();
        i.answer(&r.id, &Answer { allow: Some(true), ..Default::default() }).unwrap();
    });
    assert!(asked.load(Ordering::SeqCst));
    assert!(f.seen.lock().unwrap().is_empty());
    f.assert_invalidated_and_restudied();
}

#[test]
fn review_fix_unindexed_private_route_is_still_reviewed() {
    let mut f = Fixture::new(false);
    f.save_account("fixture-account-1");
    f.route("account", true, "fixture-account-1");
    std::fs::remove_file(f.ws.capture_dir().join("routes.json")).unwrap();
    let asked = Arc::new(AtomicBool::new(false));
    let asked2 = asked.clone();
    f.run(move |i, r| {
        asked2.store(true, Ordering::SeqCst);
        assert!(r.capture_review.as_ref().unwrap().screenshots.iter().all(|s| s.file.starts_with("routes/account/")));
        i.answer(&r.id, &Answer { allow: Some(false), ..Default::default() }).unwrap();
    });
    assert!(asked.load(Ordering::SeqCst));
    assert!(f.seen.lock().unwrap().is_empty());
}

// Browser callbacks are synthetic; the worker, registered-tool dispatch, files, inbox and first
// builder turn are real. No CDP server, real profile, browser process or network is used.
#[derive(Clone, Default)]
struct SyntheticBrowser {
    url: String,
    signed_in: bool,
    revision: Option<String>,
    endpoint: String,
}
#[derive(Default)]
struct DeepStudyFixture {
    change_after_routes: bool,
    replace_live_after_reopen: bool,
    reopen_error: bool,
    cancel_at: Option<&'static str>,
    cancel_token: bool,
    opens: usize,
    captures: usize,
    live: SyntheticBrowser,
    browsers: HashMap<String, SyntheticBrowser>,
    reads: Vec<(String, bool)>,
    optional_routes: bool,
    route_open_error: Option<ErrorCode>,
    route_capture_error: Option<ErrorCode>,
    route_identity_failure: Option<&'static str>,
    existing_route: Option<&'static str>,
    capture_urls: Vec<String>,
    capture_paths: Vec<PathBuf>,
}

fn synthetic_account(session: &craft_anything_engine::Session, revision: &str) {
    let dir = craft_anything_engine::tools::site_sign_in::sign_ins_dir(session).join("site-example.test_443");
    std::fs::create_dir_all(dir.join("profile")).unwrap();
    std::fs::write(dir.join("sign-in.json"), serde_json::to_vec(&json!({"version":1,"site":"site-example.test_443","revision":revision})).unwrap()).unwrap();
}

fn synthetic_study_tool(ctx: &mut craft_anything_engine::Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    use craft_anything_engine::tools::site_sign_in;
    let state = ctx.session.ext.get::<Mutex<DeepStudyFixture>>().unwrap();
    let mut state = state.lock().unwrap();
    let deep_open = ctx.tool == "web_site_open" && state.opens % 3 == 2;
    if state.cancel_at == Some(ctx.tool) && (ctx.tool != "web_site_open" || deep_open) {
        if state.cancel_token {
            ctx.cancel.cancel();
        }
        return Err(ToolError::cancelled());
    }
    match ctx.tool {
        "web_site_open" => {
            state.opens += 1;
            if params["url"].as_str().is_some_and(|u| u.ends_with("/broken"))
                && let Some(code) = state.route_open_error
            {
                // Failed navigation leaves a session that must never be captured as this route.
                state.live.url = "https://wrong.test/unintended".into();
                return Err(ToolError::new(code, "Synthetic optional route did not open"));
            }
            if deep_open && state.reopen_error {
                // A failed navigation/start may leave an older signed-in session alive.
                state.live.signed_in = true;
                return Err(ToolError::new(ErrorCode::EngineMissing, "Synthetic reopen failed"));
            }
            let saved = site_sign_in::saved(ctx.session, "site-example.test_443");
            let signed_in = params.get("signed_in").and_then(Value::as_bool).unwrap_or(saved.is_some());
            let browser = SyntheticBrowser {
                url: params["url"].as_str().unwrap().to_string(),
                signed_in,
                revision: if signed_in { Some(saved.unwrap().revision) } else { None },
                endpoint: format!("http://127.0.0.1:{}", 41000 + state.opens),
            };
            state.browsers.insert(browser.endpoint.clone(), browser.clone());
            state.live = browser.clone();
            if deep_open && state.replace_live_after_reopen {
                state.live = SyntheticBrowser {
                    signed_in: true,
                    revision: Some("unintended-account".into()),
                    endpoint: "http://127.0.0.1:42000".into(),
                    ..Default::default()
                };
            }
            let mut result = json!({"url":params["url"],"signed_in":browser.signed_in,
                "signed_in_site":if browser.signed_in { Some("site-example.test_443") } else { None },
                "sign_in_revision":browser.revision,"cdp_endpoint":browser.endpoint});
            if params["url"].as_str().is_some_and(|u| u.ends_with("/broken")) {
                match state.route_identity_failure {
                    Some("revision") => result["sign_in_revision"] = json!("unexpected-account"),
                    Some("site") => result["signed_in_site"] = json!("site-wrong.test_443"),
                    Some("profile") => result["signed_in"] = json!(false),
                    Some("endpoint") => result["cdp_endpoint"] = json!("http://remote.test:9222"),
                    _ => {}
                }
            }
            Ok(ToolOutput::Value(result))
        }
        "web_site_capture" => {
            state.captures += 1;
            let dir = PathBuf::from(params["output_directory"].as_str().unwrap());
            let url = state.live.url.clone();
            state.capture_urls.push(url.clone());
            state.capture_paths.push(dir.clone());
            std::fs::create_dir_all(&dir).unwrap();
            if url.ends_with("/broken")
                && let Some(code) = state.route_capture_error
            {
                std::fs::write(dir.join("partial.tmp"), "Synthetic incomplete route").unwrap();
                return Err(ToolError::new(code, "Synthetic route capture failed after creating its folder"));
            }
            std::fs::write(dir.join("screenshot-1440.png"), encode_png(&Image { width: 2, height: 2, rgba: vec![80; 16] }).unwrap()).unwrap();
            std::fs::write(dir.join("README.md"), "Synthetic account dashboard").unwrap();
            let root = url.ends_with("/app");
            let links = if !root {
                Vec::new()
            } else if state.optional_routes {
                vec!["account", "broken", "settings"]
            } else {
                vec!["account"]
            };
            let metadata = json!({"url":url,
                "signed_in":state.live.signed_in,"signed_in_site":if state.live.signed_in { Some("site-example.test_443") } else { None },
                "sign_in_revision":state.live.revision,"sign_in_page":false,"screenshots":[{"file":"screenshot-1440.png"}],
                "links":links.iter().map(|r| json!({"href":format!("https://example.test/{r}")})).collect::<Vec<_>>() });
            std::fs::write(dir.join("capture.json"), serde_json::to_vec(&metadata).unwrap()).unwrap();
            if root && state.optional_routes {
                std::fs::write(dir.join("keep.txt"), "Unrelated root evidence").unwrap();
                if let Some(kind) = state.existing_route {
                    let existing = dir.join("routes/broken");
                    std::fs::create_dir_all(&existing).unwrap();
                    std::fs::write(existing.join("keep.txt"), "Pre-existing route evidence").unwrap();
                    if kind == "successful" {
                        std::fs::copy(dir.join("capture.json"), existing.join("capture.json")).unwrap();
                        std::fs::copy(dir.join("screenshot-1440.png"), existing.join("screenshot-1440.png")).unwrap();
                    }
                    #[cfg(unix)]
                    if kind == "link" {
                        let outside = ctx.session.config_dir().join("outside-route");
                        std::fs::create_dir_all(&outside).unwrap();
                        std::fs::write(outside.join("keep.txt"), "Outside project evidence").unwrap();
                        std::fs::remove_dir_all(&existing).unwrap();
                        std::os::unix::fs::symlink(outside, &existing).unwrap();
                    }
                }
            }
            if !root && state.change_after_routes {
                synthetic_account(ctx.session, "fixture-account-2");
            }
            Ok(ToolOutput::Value(json!({})))
        }
        "web_page_states_capture" => Ok(ToolOutput::Value(json!({}))),
        _ => {
            let browser = params.get("cdp_endpoint").and_then(Value::as_str).map(|e| state.browsers.get(e).unwrap()).unwrap_or(&state.live).clone();
            state.reads.push((ctx.tool.to_string(), browser.signed_in));
            let content = if browser.signed_in { "Synthetic PRIVATE Deep artifact" } else { "Synthetic PUBLIC Deep artifact" };
            if ctx.tool == "web_walkthrough_record" {
                use craft_anything_evidence::{Environment, EvidenceBuilder, Subject};
                let ev = EvidenceBuilder::new(
                    ctx.tool,
                    Subject { kind: "website".into(), sha256: None, name: Some("Synthetic site".into()) },
                    Environment::builtin(false),
                )
                .summary("Synthetic recording")
                .result(json!({"network":{"requests":[{"url":"https://example.test/data",
                    "response_body":{"state":"captured","text":if browser.signed_in { PRIVATE_HAR_MARKER } else { PUBLIC_HAR_MARKER }}}]}}))
                .build()
                .unwrap();
                Ok(ToolOutput::evidence(ev))
            } else {
                Ok(ToolOutput::Value(json!({"content":content})))
            }
        }
    }
}

impl Fixture {
    fn fresh_deep(signed_in: bool, state: DeepStudyFixture) -> (Self, Arc<Mutex<DeepStudyFixture>>) {
        let mut f = Self::new(signed_in);
        let optional_routes = state.optional_routes;
        let state = Arc::new(Mutex::new(state));
        let mut session = craft_anything_engine::testing::session();
        session.settings.deep_mode = true;
        session.settings.engines.tools.chromium = Some(std::env::current_exe().unwrap());
        session.settings.build.max_pages = if optional_routes { 3 } else { 1 };
        session.ext.insert(state.clone());
        let callbacks: Vec<_> =
            ["web_site_open", "web_site_capture", "web_page_states_capture", "web_page_js_analyze", "web_page_watch", "web_walkthrough_record"]
                .into_iter()
                .map(|id| (id, synthetic_study_tool as craft_anything_engine::tool::ToolFn))
                .collect();
        f.worker.engine = Arc::new(craft_anything_engine::testing::engine_with_callbacks(session, &callbacks));
        if signed_in {
            f.save_account("fixture-account-1");
        }
        f.ws.manifest.capture_deep = None;
        f.ws.save().unwrap();
        (f, state)
    }
    fn restart(&mut self) {
        self.ws = Workspace::open(&self.ws.root).unwrap();
        self.worker.stop = CancelToken::default();
        self.worker.seen.clear();
        self.seen.lock().unwrap().clear();
    }
}

#[test]
fn deep_profile_new_sign_in_cannot_upgrade_an_unsigned_study() {
    let (mut f, state) = Fixture::fresh_deep(false, DeepStudyFixture { change_after_routes: true, ..Default::default() });
    f.run(|_, _| panic!("a study that stays unsigned needs no account-content approval"));
    assert_eq!(state.lock().unwrap().captures, 2, "the real Study must capture the root and route");
    assert_eq!(f.seen.lock().unwrap().len(), 1);
    assert!(f.seen.lock().unwrap()[0].contains("Synthetic PUBLIC Deep artifact"));
    assert!(!f.seen.lock().unwrap()[0].contains("Synthetic PRIVATE Deep artifact"), "private Deep content bypassed confirmation");
    assert!(state.lock().unwrap().reads.iter().all(|(_, signed)| !signed));
}

#[test]
fn deep_profile_every_reader_stays_on_the_verified_browser() {
    let (mut f, state) = Fixture::fresh_deep(false, DeepStudyFixture { replace_live_after_reopen: true, ..Default::default() });
    f.run(|_, _| panic!("an unrelated session cannot upgrade this unsigned study"));
    let reads = &state.lock().unwrap().reads;
    assert_eq!(reads.len(), 3);
    assert!(reads.iter().all(|(_, signed)| !signed), "a Deep tool fell back to an unintended signed-in session: {reads:?}");
    assert!(!f.seen.lock().unwrap()[0].contains("Synthetic PRIVATE Deep artifact"));
}

#[test]
fn deep_profile_account_replacement_stops_and_requires_a_new_study() {
    let (mut f, state) = Fixture::fresh_deep(true, DeepStudyFixture { change_after_routes: true, ..Default::default() });
    f.run(|_, _| panic!("a different account needs a new study, not approval of old screenshots"));
    assert!(f.seen.lock().unwrap().is_empty());
    assert!(state.lock().unwrap().reads.is_empty(), "Deep inspected a replacement account");
    assert!(!f.ws.capture_dir().join("how-it-works.md").exists());
    assert_eq!(Workspace::open(&f.ws.root).unwrap().manifest.capture_deep, None);
    let before = state.lock().unwrap().captures;
    state.lock().unwrap().change_after_routes = false;
    f.restart();
    f.run(|i, r| i.answer(&r.id, &Answer { allow: Some(false), ..Default::default() }).unwrap());
    assert!(state.lock().unwrap().captures > before, "retry must study again");
    assert!(f.seen.lock().unwrap().is_empty());
}

#[test]
fn deep_profile_signed_in_study_requires_explicit_confirmation() {
    for allow in [false, true] {
        let (mut f, state) = Fixture::fresh_deep(true, DeepStudyFixture::default());
        let seen = f.seen.clone();
        let root = f.ws.root.clone();
        let asked = Arc::new(AtomicBool::new(false));
        let asked2 = asked.clone();
        f.run(move |i, r| {
            asked2.store(true, Ordering::SeqCst);
            assert!(seen.lock().unwrap().is_empty());
            assert!(std::fs::read_to_string(root.join("capture/how-it-works.md")).unwrap().contains("Synthetic PRIVATE Deep artifact"));
            assert_eq!(r.capture_review.as_ref().unwrap().screenshots.len(), 2);
            i.answer(&r.id, &Answer { allow: Some(allow), ..Default::default() }).unwrap();
        });
        assert!(asked.load(Ordering::SeqCst));
        assert_eq!(f.seen.lock().unwrap().len(), usize::from(allow));
        if allow {
            assert!(f.seen.lock().unwrap()[0].contains("Synthetic PRIVATE Deep artifact"));
        } else {
            assert_eq!(Workspace::open(&f.ws.root).unwrap().manifest.capture_deep, None);
            let before = state.lock().unwrap().captures;
            f.restart();
            f.run(|i, r| i.answer(&r.id, &Answer { allow: Some(false), ..Default::default() }).unwrap());
            assert!(state.lock().unwrap().captures > before, "a rejected study was reused");
        }
    }
}

#[test]
fn deep_profile_fully_unsigned_study_needs_no_confirmation() {
    let (mut f, state) = Fixture::fresh_deep(false, DeepStudyFixture::default());
    f.run(|_, _| panic!("fully unsigned Deep study asked for private-content confirmation"));
    assert_eq!(f.seen.lock().unwrap().len(), 1);
    assert!(state.lock().unwrap().reads.iter().all(|(_, signed)| !signed));
    assert!(f.seen.lock().unwrap()[0].contains("Synthetic PUBLIC Deep artifact"));
}

#[test]
fn deep_profile_reopen_errors_do_not_inspect_a_leftover_browser() {
    let (mut f, state) = Fixture::fresh_deep(false, DeepStudyFixture { reopen_error: true, ..Default::default() });
    let err = f.run(|_, _| panic!("failed reopening cannot authorize the old session"));
    assert_eq!(err.code, ErrorCode::EngineMissing);
    assert!(state.lock().unwrap().reads.is_empty());
    assert!(f.seen.lock().unwrap().is_empty());
    assert!(!f.ws.capture_dir().join("how-it-works.md").exists());
    assert_eq!(Workspace::open(&f.ws.root).unwrap().manifest.capture_deep, None);
}

#[test]
fn deep_profile_cancellation_stops_following_readers_and_publication() {
    for tool in ["web_site_open", "web_page_js_analyze", "web_page_watch", "web_walkthrough_record"] {
        for cancel_token in [false, true] {
            let (mut f, state) = Fixture::fresh_deep(false, DeepStudyFixture { cancel_at: Some(tool), cancel_token, ..Default::default() });
            let err = f.run(|_, _| panic!("cancelled study cannot authorize content"));
            assert_eq!(err.code, ErrorCode::Cancelled);
            let before = match tool {
                "web_site_open" | "web_page_js_analyze" => 0,
                "web_page_watch" => 1,
                _ => 2,
            };
            assert_eq!(state.lock().unwrap().reads.len(), before, "tools ran after cancellation at {tool}");
            assert!(f.seen.lock().unwrap().is_empty());
            assert!(!f.ws.capture_dir().join("how-it-works.md").exists());
            assert_eq!(Workspace::open(&f.ws.root).unwrap().manifest.capture_deep, None);
        }
    }
}

#[test]
fn deep_profile_unattested_legacy_artifacts_are_invalidated_before_build() {
    for signed_in in [false, true] {
        let mut f = Fixture::new(signed_in);
        f.worker.engine.session().settings.deep_mode = true;
        f.ws.manifest.capture_deep = Some(true);
        f.ws.save().unwrap();
        std::fs::write(f.ws.capture_dir().join("how-it-works.md"), "Synthetic PRIVATE Deep artifact with unknown profile").unwrap();
        f.run(|_, _| panic!("legacy Deep artifacts have no trustworthy profile provenance"));
        assert!(f.seen.lock().unwrap().is_empty());
        f.assert_invalidated_and_restudied();
    }
}

const PRIVATE_HAR_MARKER: &str = "Synthetic PRIVATE HAR response only";
const PUBLIC_HAR_MARKER: &str = "Synthetic PUBLIC HAR response only";

#[test]
fn route_review_failed_partial_capture_is_removed_without_losing_successes() {
    for signed_in in [false, true] {
        let (mut f, state) = Fixture::fresh_deep(
            signed_in,
            DeepStudyFixture { optional_routes: true, route_capture_error: Some(ErrorCode::ProgramFailed), ..Default::default() },
        );
        let (tx, rx) = std::sync::mpsc::channel();
        f.worker.tx = tx;
        f.run(|i, r| i.answer(&r.id, &Answer { allow: Some(true), ..Default::default() }).unwrap());
        assert!(!f.ws.capture_dir().join("routes/broken").exists(), "a failed optional capture left a partial route");
        assert_eq!(f.seen.lock().unwrap().len(), 1, "a failed optional route prevented a valid build");
        for name in ["account", "settings"] {
            assert!(f.ws.capture_dir().join("routes").join(name).join("capture.json").is_file());
            assert_eq!(std::fs::read_to_string(f.ws.capture_dir().join("routes").join(name).join("README.md")).unwrap(), "Synthetic account dashboard");
        }
        assert_eq!(std::fs::read_to_string(f.ws.capture_dir().join("keep.txt")).unwrap(), "Unrelated root evidence");
        let kept: Vec<kinds::website::Route> = serde_json::from_slice(&std::fs::read(f.ws.capture_dir().join("routes.json")).unwrap()).unwrap();
        assert!(kept.iter().all(|r| r.slug != "broken"));
        assert_eq!(kept.len(), 2);
        assert!(state.lock().unwrap().capture_paths.iter().all(|p| !p.join("partial.tmp").exists()), "failed attempt storage was retained");
        assert!(rx.try_iter().any(|e| matches!(e,RunEvent::Step {text,..} if text.contains("Couldn't photograph /broken"))));
    }
}

#[test]
fn route_review_navigation_failure_is_reported_without_capturing_the_old_session() {
    let (mut f, state) =
        Fixture::fresh_deep(false, DeepStudyFixture { optional_routes: true, route_open_error: Some(ErrorCode::ProgramFailed), ..Default::default() });
    let (tx, rx) = std::sync::mpsc::channel();
    f.worker.tx = tx;
    f.run(|_, _| panic!("a fully unsigned study needs no approval"));
    assert_eq!(f.seen.lock().unwrap().len(), 1, "ordinary linked-page navigation failure stopped Study");
    assert!(state.lock().unwrap().capture_urls.iter().all(|u| !u.ends_with("/broken") && !u.contains("unintended")));
    assert!(f.ws.capture_dir().join("routes/settings/capture.json").is_file());
    assert!(!f.ws.capture_dir().join("routes/broken").exists());
    assert!(rx.try_iter().any(|e| matches!(e,RunEvent::Step {text,..} if text.contains("/broken")&&text.contains("Synthetic optional route did not open"))));
}

#[test]
fn route_review_navigation_integrity_errors_are_not_optional() {
    for code in [
        ErrorCode::Cancelled,
        ErrorCode::NoPermission,
        ErrorCode::StaleRevision,
        ErrorCode::ConsentRequired,
        ErrorCode::FileChanged,
        ErrorCode::EvidenceHashMismatch,
    ] {
        let (mut f, state) = Fixture::fresh_deep(true, DeepStudyFixture { optional_routes: true, route_open_error: Some(code), ..Default::default() });
        let err = f.run(|_, _| panic!("integrity errors cannot be approved"));
        assert_eq!(err.code, code);
        assert!(f.seen.lock().unwrap().is_empty());
        assert!(state.lock().unwrap().reads.is_empty());
        assert!(state.lock().unwrap().capture_urls.iter().all(|u| !u.ends_with("/broken") && !u.ends_with("/settings") && !u.contains("unintended")));
        assert_eq!(Workspace::open(&f.ws.root).unwrap().manifest.capture_deep, None);
    }
}

#[test]
fn route_review_successful_opens_still_require_the_original_identity() {
    for kind in ["revision", "site", "profile", "endpoint"] {
        let (mut f, state) = Fixture::fresh_deep(true, DeepStudyFixture { optional_routes: true, route_identity_failure: Some(kind), ..Default::default() });
        let err = f.run(|_, _| panic!("changed browser identities cannot be approved"));
        assert_eq!(err.code, ErrorCode::NoPermission);
        assert!(f.seen.lock().unwrap().is_empty());
        assert!(state.lock().unwrap().capture_urls.iter().all(|u| !u.ends_with("/broken") && !u.ends_with("/settings")));
        assert!(state.lock().unwrap().reads.is_empty());
        assert_eq!(Workspace::open(&f.ws.root).unwrap().manifest.capture_deep, None);
    }
}

#[test]
fn route_review_capture_integrity_errors_stop_before_the_next_route() {
    for code in [
        ErrorCode::NoPermission,
        ErrorCode::Cancelled,
        ErrorCode::StaleRevision,
        ErrorCode::FileHashMismatch,
        ErrorCode::EvidenceHashMismatch,
        ErrorCode::ConsentRequired,
    ] {
        let (mut f, state) = Fixture::fresh_deep(true, DeepStudyFixture { optional_routes: true, route_capture_error: Some(code), ..Default::default() });
        let err = f.run(|_, _| panic!("capture integrity errors cannot be approved"));
        assert_eq!(err.code, code);
        assert!(state.lock().unwrap().capture_urls.iter().all(|u| !u.ends_with("/settings")), "continued after {code:?}");
        assert!(f.seen.lock().unwrap().is_empty());
        assert!(state.lock().unwrap().reads.is_empty());
        assert!(!f.ws.capture_dir().join("routes/broken").exists());
        assert!(state.lock().unwrap().capture_paths.iter().all(|p| !p.join("partial.tmp").exists()));
        assert_eq!(Workspace::open(&f.ws.root).unwrap().manifest.capture_deep, None);
    }
}

#[test]
fn route_review_unexpected_route_folders_are_preserved_and_fail_closed() {
    for kind in ["partial", "successful"] {
        let (mut f, _) = Fixture::fresh_deep(false, DeepStudyFixture { optional_routes: true, existing_route: Some(kind), ..Default::default() });
        f.run(|_, _| panic!("unexpected route folders must not be silently reused"));
        assert!(f.seen.lock().unwrap().is_empty());
        assert_eq!(std::fs::read_to_string(f.ws.capture_dir().join("routes/broken/keep.txt")).unwrap(), "Pre-existing route evidence");
        if kind == "successful" {
            assert!(f.ws.capture_dir().join("routes/broken/capture.json").is_file());
        }
        assert_eq!(Workspace::open(&f.ws.root).unwrap().manifest.capture_deep, None);
    }
}

#[cfg(unix)]
#[test]
fn route_review_unexpected_route_links_are_not_followed_or_removed() {
    let (mut f, _) = Fixture::fresh_deep(false, DeepStudyFixture { optional_routes: true, existing_route: Some("link"), ..Default::default() });
    f.run(|_, _| panic!("route links cannot be approved"));
    assert!(f.seen.lock().unwrap().is_empty());
    assert!(std::fs::symlink_metadata(f.ws.capture_dir().join("routes/broken")).unwrap().file_type().is_symlink());
    let outside = f.worker.engine.session().config_dir().join("outside-route/keep.txt");
    assert_eq!(std::fs::read_to_string(outside).unwrap(), "Outside project evidence");
}

#[test]
fn route_review_har_body_is_retained_and_released_only_after_confirmation() {
    for allow in [false, true] {
        let (mut f, _) = Fixture::fresh_deep(true, DeepStudyFixture::default());
        let root = f.ws.root.clone();
        let seen = f.seen.clone();
        let asked = Arc::new(AtomicBool::new(false));
        let asked2 = asked.clone();
        f.run(move |i, r| {
            asked2.store(true, Ordering::SeqCst);
            assert!(seen.lock().unwrap().is_empty(), "the builder read account content before confirmation");
            let notes = std::fs::read_to_string(root.join("capture/how-it-works.md")).unwrap();
            assert!(notes.contains("Synthetic PRIVATE Deep artifact"));
            assert!(!notes.contains(PRIVATE_HAR_MARKER), "HAR assertions must not pass from the notes");
            let raw = std::fs::read_to_string(root.join("capture/original.har")).unwrap();
            let har: Value = serde_json::from_str(&raw).unwrap();
            assert_eq!(har.pointer("/log/entries/0/response/content/text").and_then(Value::as_str), Some(PRIVATE_HAR_MARKER));
            assert!(raw.contains(PRIVATE_HAR_MARKER));
            assert!(!raw.contains(PUBLIC_HAR_MARKER));
            i.answer(&r.id, &Answer { allow: Some(allow), ..Default::default() }).unwrap();
        });
        assert!(asked.load(Ordering::SeqCst));
        assert_eq!(f.seen.lock().unwrap().len(), usize::from(allow));
        let observed = f.seen.lock().unwrap();
        assert_eq!(observed.iter().any(|s| s.contains(PRIVATE_HAR_MARKER)), allow);
    }
}
