//! Several rebuilds at once. A [`RunSet`] keeps every project's run by folder: up to its limit
//! work at the same time, the rest wait in order and start as others finish. The app and the
//! rebuild tools both hold one; each run keeps its own project's feed, so nothing here depends on
//! which project is on screen. A run whose AI uses our own agent loop works alone: its tools run in
//! the app's session, which points at one project at a time, so a run starting beside it would send
//! its files and asset spending to the other project.

use super::*;
use std::collections::{HashMap, VecDeque};

/// How a prepared run is started; the built-in worker unless a fork or a test swaps it.
/// `RunSet` has already restored its request and owns cleanup of the saved record.
pub type Starter = Box<dyn FnMut(&AiHub, Workspace, RunOptions) -> Result<RebuildRun, ToolError> + Send>;

/// What [`RunSet::start`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Started {
    Running,
    /// Waiting for a free place; 1 starts next.
    Queued {
        position: usize,
    },
}

/// What [`RunSet::stop`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stopped {
    /// Asked to stop; its `Finished` event follows.
    Stopping,
    /// Paused before it started. Its request stays for the next manual start.
    Unqueued,
    Nothing,
}

pub struct RunSet {
    limit: usize,
    /// Running and finished runs (a finished one keeps its preview and unsent notes).
    runs: HashMap<PathBuf, RebuildRun>,
    /// Waiting runs, and whether each last looked like it must work alone: `poll` runs every frame
    /// and looking it up can read the settings file, so it's looked up again only when a run may
    /// be about to start.
    queue: VecDeque<(Workspace, RunOptions, bool)>,
    /// The latest run to start, if it had to work alone; nothing else starts while it's working.
    alone: Option<PathBuf>,
    starter: Starter,
    /// A [`keep_queue_moving`] thread is looking after the queue.
    watched: bool,
}

/// Whether this run's tools would run in the app's session: its Builder is an API backend, or the
/// Extractor that only works on a fresh study is (the same test as [`RebuildRun::start`]). Agent
/// apps have a tool server of their own.
fn must_work_alone(hub: &AiHub, ws: &Workspace, opts: &RunOptions) -> bool {
    let cfg = hub.config();
    let in_session = |e: &crate::BackendEntry| e.config.kind != BackendKind::AgentCli;
    let fresh = opts.message.is_none() || !super::capture_in_mode(ws, super::run_deep_mode(hub, opts));
    builder_backend(&cfg, opts.backend.as_deref().or_else(|| super::project_builder(&cfg, ws))).is_ok_and(|e| in_session(&e))
        || (fresh && hub.chosen(Role::Extractor).and_then(|n| cfg.get(&n).cloned()).is_some_and(|e| in_session(&e)))
}

impl std::fmt::Debug for RunSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunSet").field("limit", &self.limit).field("running", &self.active()).field("queued", &self.queue.len()).finish()
    }
}

impl RunSet {
    /// At most `limit` runs work at once (at least one).
    pub fn new(limit: usize) -> RunSet {
        RunSet { limit: limit.max(1), runs: HashMap::new(), queue: VecDeque::new(), alone: None, starter: Box::new(RebuildRun::start_prepared), watched: false }
    }

    /// A run may start now: there's a place, and neither it nor anyone working needs to be alone.
    fn has_room(&self, alone: bool) -> bool {
        let active = self.active();
        active < self.limit && (!alone || active == 0) && !self.alone.as_ref().is_some_and(|root| self.is_running(root))
    }

    /// Start runs another way (a fork's runner, or a test's).
    pub fn with_starter(mut self, starter: Starter) -> RunSet {
        self.starter = starter;
        self
    }

    /// Change how many may work at once; runs already working go on.
    pub fn set_limit(&mut self, limit: usize) {
        self.limit = limit.max(1);
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Start a run on `ws`, or queue it when `limit` are already working or it has to wait to work
    /// alone. Nothing overtakes the queue, so one waiting to be alone gets its turn. A project that
    /// is working or queued is refused.
    pub fn start(&mut self, hub: &AiHub, mut ws: Workspace, mut opts: RunOptions) -> Result<Started, ToolError> {
        if self.is_running(&ws.root) || self.position(&ws.root).is_some() {
            return Err(
                ToolError::new(ErrorCode::BadInput, "This project is already building.").with_remediation("Add to it while it works, or stop it first.")
            );
        }
        // Queueing saves its waiting status too. Keep the last run's final session and builder
        // choices, rather than writing them back from the caller's older project snapshot.
        ws.reload()?;
        let saved = queued_request::restore(&ws, &mut opts)?;
        let alone = must_work_alone(hub, &ws, &opts);
        if self.queue.is_empty() && self.has_room(alone) {
            let root = ws.root.clone();
            let run = (self.starter)(hub, ws.clone(), opts)?;
            queued_request::finished(&ws, saved.as_ref());
            self.alone = alone.then(|| root.clone());
            self.runs.insert(root, run);
            return Ok(Started::Running);
        }
        // Fail now, not when its turn comes, when no coding agent is set up.
        let cfg = hub.config();
        builder_backend(&cfg, opts.backend.as_deref().or_else(|| super::project_builder(&cfg, &ws)))?;
        queued_request::save(&ws, &opts)?;
        ws.set_status(Status::Stopped, "Waiting for a build slot.")?;
        self.queue.push_back((ws, opts, alone));
        Ok(Started::Queued { position: self.queue.len() })
    }

    /// Every run's new events, by project folder, then start queued runs in the places that came
    /// free. A queued run that can't start gives a `Finished { ok: false }` with the reason.
    pub fn poll(&mut self, hub: &AiHub) -> Vec<(PathBuf, RunEvent)> {
        let mut out: Vec<(PathBuf, RunEvent)> = Vec::new();
        for (root, run) in &mut self.runs {
            out.extend(run.poll().into_iter().map(|e| (root.clone(), e)));
        }
        while self.queue.front().is_some_and(|(_, _, alone)| self.has_room(*alone)) {
            let Some((ws, opts, _)) = self.queue.pop_front() else { break };
            // The project as it is now, with the saved session from its preceding run.
            let ws = Workspace::open(&ws.root).unwrap_or(ws);
            // The Builder may have been changed while it waited.
            let alone = must_work_alone(hub, &ws, &opts);
            if !self.has_room(alone) {
                self.queue.push_front((ws, opts, alone));
                break;
            }
            let root = ws.root.clone();
            let mut keep = ws.clone();
            let saved = queued_request::QueuedRequest::from_options(&opts);
            let started = queued_request::validate(&ws).and_then(|()| (self.starter)(hub, ws, opts));
            match started {
                Ok(run) => {
                    queued_request::finished(&keep, saved.as_ref());
                    self.alone = alone.then(|| root.clone());
                    self.runs.insert(root, run);
                }
                Err(e) => {
                    let message = e.for_people();
                    let _ = keep.set_status(Status::NeedsYou, message.clone());
                    out.push((root, RunEvent::Finished { ok: false, message, paused: false }));
                }
            }
        }
        out
    }

    /// The project's run, working or finished.
    pub fn get(&self, root: &Path) -> Option<&RebuildRun> {
        self.runs.get(root)
    }

    pub fn get_mut(&mut self, root: &Path) -> Option<&mut RebuildRun> {
        self.runs.get_mut(root)
    }

    /// The project's run is working.
    pub fn is_running(&self, root: &Path) -> bool {
        self.runs.get(root).is_some_and(RebuildRun::is_running)
    }

    /// Its place in the queue (1 starts next).
    pub fn position(&self, root: &Path) -> Option<usize> {
        self.queue.iter().position(|(ws, _, _)| ws.root == root).map(|i| i + 1)
    }

    /// How many are waiting.
    pub fn waiting(&self) -> usize {
        self.queue.len()
    }

    /// Working or waiting.
    pub fn is_busy(&self, root: &Path) -> bool {
        self.is_running(root) || self.position(root).is_some()
    }

    /// How many are working.
    pub fn active(&self) -> usize {
        self.runs.values().filter(|r| r.is_running()).count()
    }

    /// Any project is working or waiting.
    pub fn any_busy(&self) -> bool {
        self.active() > 0 || !self.queue.is_empty()
    }

    /// The projects working or waiting.
    pub fn busy(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self.runs.iter().filter(|(_, r)| r.is_running()).map(|(p, _)| p.clone()).collect();
        v.extend(self.queue.iter().map(|(ws, _, _)| ws.root.clone()));
        v
    }

    /// Stop the project's run, or take it out of the queue.
    pub fn stop(&mut self, root: &Path) -> Stopped {
        if let Some(i) = self.queue.iter().position(|(ws, _, _)| ws.root == root) {
            self.queue.remove(i);
            return Stopped::Unqueued;
        }
        match self.runs.get(root).filter(|r| r.is_running()) {
            Some(r) => {
                r.stop();
                Stopped::Stopping
            }
            None => Stopped::Nothing,
        }
    }

    /// Empty the queue and stop every run (closing the app).
    pub fn stop_all(&mut self) {
        self.queue.clear();
        for r in self.runs.values() {
            r.stop();
        }
    }
}

/// Poll `set` every `every` on a background thread while runs wait, so queued runs start as
/// soon as a place comes free: for holders that poll only when called (the rebuild tools, which
/// an MCP client may not call again for a while). `settings_path` is the session's config file,
/// read when the session is busy. One thread per set at a time; it ends when the queue is empty
/// or the engine is gone, and never waits on a busy session.
pub fn keep_queue_moving(set: &Arc<Mutex<RunSet>>, engine: &Arc<Engine>, settings_path: PathBuf, every: Duration) {
    {
        let mut s = set.lock().unwrap_or_else(PoisonError::into_inner);
        if s.watched || s.queue.is_empty() {
            return;
        }
        s.watched = true;
    }
    let (set_ref, engine_ref) = (Arc::downgrade(set), Arc::downgrade(engine));
    let spawned = std::thread::Builder::new().name("rebuild-queue".into()).spawn(move || {
        loop {
            std::thread::sleep(every);
            let Some(set) = set_ref.upgrade() else { return };
            let mut s = set.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(engine) = engine_ref.upgrade() else {
                s.watched = false;
                return;
            };
            if let Some(session) = engine.try_session() {
                s.set_limit(session.settings.build.parallel);
            }
            // Each run keeps its own project's feed, so the events aren't needed here.
            let hub = AiHub::with_stores(engine, settings_path.clone(), crate::KeyStore::system());
            let _ = s.poll(&hub);
            if s.queue.is_empty() {
                s.watched = false;
                return;
            }
        }
    });
    if spawned.is_err() {
        set.lock().unwrap_or_else(PoisonError::into_inner).watched = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rebuild::StandIns;

    /// A hub whose Builder is an agent app, so queued runs pass the up-front check.
    fn hub() -> AiHub {
        let mut s = craft_anything_engine::testing::session();
        s.settings.ai = toml::from_str("[[backends]]\nname = \"Agent\"\nkind = \"agent_cli\"\nmodel = \"claude-code\"\n").unwrap();
        assert!(builder_backend(&crate::AiConfig::from_settings(&s.settings), None).is_ok());
        AiHub::new(Arc::new(Engine::new(s)))
    }

    type SeenRequest = (Option<String>, Option<String>, Vec<PathBuf>);

    fn remembering_requests(seen: Arc<Mutex<Vec<SeenRequest>>>) -> Starter {
        let mut start = StandIns::new().starter();
        Box::new(move |hub, ws, opts| {
            seen.lock().unwrap().push((opts.message.clone(), opts.shown.clone(), opts.shown_pictures.clone()));
            start(hub, ws, opts)
        })
    }

    #[test]
    fn a_queued_request_survives_shutdown_and_an_abruptly_lost_queue() {
        for stopped in [false, true] {
            let d = tempfile::tempdir().unwrap();
            let a = Workspace::create(d.path(), "A", "A game", TargetRef::None).unwrap();
            let b = Workspace::create(d.path(), "B", "A game", TargetRef::None).unwrap();
            let picture = b.root.join("pictures/reference.png");
            let opts = RunOptions {
                message: Some("Keep the exact request\nwith its attachment path".into()),
                shown: Some("Keep the exact request".into()),
                shown_pictures: vec![picture.clone()],
                ..RunOptions::default()
            };
            let hub = hub();
            let mut set = RunSet::new(1).with_starter(StandIns::new().starter());
            set.start(&hub, a, RunOptions::default()).unwrap();
            assert_eq!(set.start(&hub, b.clone(), opts).unwrap(), Started::Queued { position: 1 });
            assert_eq!(Workspace::open(&b.root).unwrap().manifest.status, Status::Stopped, "a reopened project offers Continue");
            if stopped {
                set.stop_all();
            }
            drop(set);

            let seen = Arc::new(Mutex::new(Vec::new()));
            let mut reopened = RunSet::new(1).with_starter(remembering_requests(seen.clone()));
            assert_eq!(reopened.start(&hub, Workspace::open(&b.root).unwrap(), RunOptions::default()).unwrap(), Started::Running);
            assert_eq!(
                *seen.lock().unwrap(),
                [(Some("Keep the exact request\nwith its attachment path".into()), Some("Keep the exact request".into()), vec![picture])],
                "the next run receives the request, its display text and its pictures after the queue is gone"
            );
        }
    }

    #[test]
    fn a_queued_request_is_not_accepted_when_it_cannot_be_saved() {
        let d = tempfile::tempdir().unwrap();
        let a = Workspace::create(d.path(), "A", "A game", TargetRef::None).unwrap();
        let b = Workspace::create(d.path(), "B", "A game", TargetRef::None).unwrap();
        std::fs::write(b.inbox_dir(), b"not a folder").unwrap();
        let hub = hub();
        let mut set = RunSet::new(1).with_starter(StandIns::new().starter());
        set.start(&hub, a, RunOptions::default()).unwrap();
        let result = set.start(&hub, b.clone(), RunOptions { message: Some("Keep this request".into()), ..RunOptions::default() });
        assert!(result.is_err(), "an accepted request must outlive the in-memory queue");
        assert_eq!(set.position(&b.root), None, "a failed save must not enqueue the request");
    }

    #[test]
    fn a_queued_request_survives_a_failure_to_start() {
        let d = tempfile::tempdir().unwrap();
        let a = Workspace::create(d.path(), "A", "A game", TargetRef::None).unwrap();
        let b = Workspace::create(d.path(), "B", "A game", TargetRef::None).unwrap();
        let hub = hub();
        let fake = StandIns::new().failing("B");
        let mut set = RunSet::new(1).with_starter(fake.starter());
        set.start(&hub, a.clone(), RunOptions::default()).unwrap();
        set.start(&hub, b.clone(), RunOptions { message: Some("Retry this request".into()), ..RunOptions::default() }).unwrap();
        fake.finish(&a.root);
        assert!(set.poll(&hub).iter().any(|(root, e)| root == &b.root && matches!(e, RunEvent::Finished { ok: false, .. })));
        drop(set);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut reopened = RunSet::new(1).with_starter(remembering_requests(seen.clone()));
        reopened.start(&hub, b, RunOptions::default()).unwrap();
        assert_eq!(seen.lock().unwrap()[0].0.as_deref(), Some("Retry this request"));
    }

    #[test]
    fn a_changed_unreadable_request_stops_its_queued_start_and_leaves_the_next_project_free() {
        for (case, contents) in
            [("malformed", Some(b"{broken".as_slice())), ("newer", Some(br#"{"version":3,"message":"A later request"}"#.as_slice())), ("unreadable", None)]
        {
            let d = tempfile::tempdir().unwrap();
            let blocker = Workspace::create(d.path(), "Blocker", "A game", TargetRef::None).unwrap();
            let queued = Workspace::create(d.path(), "Queued", "A game", TargetRef::None).unwrap();
            let next = Workspace::create(d.path(), "Next", "A game", TargetRef::None).unwrap();
            let hub = hub();
            let fake = StandIns::new();
            let mut set = RunSet::new(1).with_starter(fake.starter());
            set.start(&hub, blocker.clone(), RunOptions::default()).unwrap();
            let opts = RunOptions { message: Some("The queued request".into()), ..Default::default() };
            assert_eq!(set.start(&hub, queued.clone(), opts).unwrap(), Started::Queued { position: 1 });
            assert_eq!(set.start(&hub, next.clone(), RunOptions::default()).unwrap(), Started::Queued { position: 2 });
            let record = queued.inbox_dir().join("queued-request.json");
            if let Some(contents) = contents {
                std::fs::write(&record, contents).unwrap();
            } else {
                std::fs::remove_file(&record).unwrap();
                std::fs::create_dir(&record).unwrap();
                std::fs::write(record.join("keep"), b"leave unchanged").unwrap();
            }
            fake.finish(&blocker.root);
            let events = set.poll(&hub);
            assert!(
                events.iter().any(|(root, event)| {
                    root == &queued.root && matches!(event, RunEvent::Finished { ok: false, message, paused: false } if message.contains("queued request"))
                }),
                "{case}: the existing failed-start path must report the record error"
            );
            assert_eq!(fake.started(), [blocker.root.clone(), next.root.clone()], "{case}: the unreadable request must not reach the starter");
            assert!(set.get(&queued.root).is_none() && set.position(&queued.root).is_none());
            assert_eq!(Workspace::open(&queued.root).unwrap().manifest.status, Status::NeedsYou);
            if let Some(contents) = contents {
                assert_eq!(std::fs::read(&record).unwrap(), contents, "{case}: the saved bytes remain unchanged");
            } else {
                assert_eq!(std::fs::read(record.join("keep")).unwrap(), b"leave unchanged");
            }
        }
    }

    #[test]
    fn a_successfully_started_request_is_not_replayed() {
        let d = tempfile::tempdir().unwrap();
        let a = Workspace::create(d.path(), "A", "A game", TargetRef::None).unwrap();
        let b = Workspace::create(d.path(), "B", "A game", TargetRef::None).unwrap();
        let hub = hub();
        let fake = StandIns::new();
        let mut start = fake.starter();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let requests = seen.clone();
        let mut set = RunSet::new(1).with_starter(Box::new(move |hub, ws, opts| {
            requests.lock().unwrap().push(opts.message.clone());
            start(hub, ws, opts)
        }));
        set.start(&hub, a.clone(), RunOptions::default()).unwrap();
        set.start(&hub, b.clone(), RunOptions { message: Some("Send once".into()), ..RunOptions::default() }).unwrap();
        fake.finish(&a.root);
        set.poll(&hub);
        assert_eq!(*seen.lock().unwrap(), [None, Some("Send once".into())]);
        drop(set);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut reopened = RunSet::new(1).with_starter(remembering_requests(seen.clone()));
        reopened.start(&hub, b, RunOptions::default()).unwrap();
        assert_eq!(seen.lock().unwrap()[0].0, None);
    }

    #[test]
    fn the_default_starter_keeps_a_request_saved_after_queueing() {
        let d = tempfile::tempdir().unwrap();
        let blocker = Workspace::create(d.path(), "Blocker", "A game", TargetRef::None).unwrap();
        let ws = Workspace::create(d.path(), "Queued", "A game", TargetRef::None).unwrap();
        let hub = AiHub::with_stores(hub().engine.clone(), d.path().join("settings.toml"), crate::KeyStore::file_only(d.path().join("keys.toml")));
        let fake = StandIns::new();
        let mut set = RunSet::new(1);
        // Only the occupied slot is a stand-in. Poll must use the real default starter for A.
        set.runs.insert(blocker.root.clone(), fake.starter()(&hub, blocker.clone(), RunOptions::default()).unwrap());
        let opts = RunOptions { message: Some("Queued request A".into()), shown: Some("Queued request A".into()), ..Default::default() };
        assert_eq!(set.start(&hub, ws.clone(), opts).unwrap(), Started::Queued { position: 1 });
        queued_request::save(&ws, &RunOptions { message: Some("Later request B".into()), ..Default::default() }).unwrap();
        fake.finish(&blocker.root);
        set.poll(&hub);
        let run = set.get_mut(&ws.root).expect("the queued run started");
        assert!(run.feed_now().iter().any(|item| matches!(item, feed::FeedItem::User(user) if user.text() == "Queued request A")));
        run.stop();
        let deadline = Instant::now() + Duration::from_secs(30);
        while run.is_running() {
            assert!(Instant::now() < deadline, "the run did not stop");
            let _ = run.rx.recv_timeout(Duration::from_millis(50));
        }
        let mut next = RunOptions::default();
        queued_request::restore(&ws, &mut next).unwrap();
        assert_eq!(next.message.as_deref(), Some("Later request B"), "starting A must not retire B's backup");
    }

    #[test]
    fn queueing_keeps_the_last_runs_saved_session() {
        let d = tempfile::tempdir().unwrap();
        let a = Workspace::create(d.path(), "A", "A game", TargetRef::None).unwrap();
        let b = Workspace::create(d.path(), "B", "A game", TargetRef::None).unwrap();
        let mut current = Workspace::open(&b.root).unwrap();
        current.manifest.agent_session = Some("the-last-runs-session".into());
        current.save().unwrap();
        current.set_person(|m| m.builder = Some("Agent".into())).unwrap();
        let hub = hub();
        let mut set = RunSet::new(1).with_starter(StandIns::new().starter());
        set.start(&hub, a, RunOptions::default()).unwrap();
        assert_eq!(
            set.start(&hub, b.clone(), RunOptions { message: Some("Keep going".into()), ..Default::default() }).unwrap(),
            Started::Queued { position: 1 }
        );
        let saved = Workspace::open(&b.root).unwrap();
        assert_eq!(saved.manifest.agent_session.as_deref(), Some("the-last-runs-session"));
        assert_eq!(saved.manifest.builder.as_deref(), Some("Agent"));
        assert_eq!(saved.manifest.status, Status::Stopped);
    }

    #[test]
    fn pausing_a_queued_request_keeps_it_for_manual_continue() {
        let d = tempfile::tempdir().unwrap();
        let a = Workspace::create(d.path(), "A", "A game", TargetRef::None).unwrap();
        let b = Workspace::create(d.path(), "B", "A game", TargetRef::None).unwrap();
        let hub = hub();
        let mut set = RunSet::new(1).with_starter(StandIns::new().starter());
        set.start(&hub, a, RunOptions::default()).unwrap();
        set.start(&hub, b.clone(), RunOptions { message: Some("Resume my change".into()), ..Default::default() }).unwrap();
        assert_eq!(set.stop(&b.root), Stopped::Unqueued);
        assert_eq!(set.position(&b.root), None);
        // Continuing while the other build still works must keep the restored words when it is
        // queued a second time, and survive another restart before a slot comes free.
        assert_eq!(set.start(&hub, b.clone(), RunOptions::default()).unwrap(), Started::Queued { position: 1 });
        drop(set);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut reopened = RunSet::new(1).with_starter(remembering_requests(seen.clone()));
        reopened.start(&hub, b, RunOptions::default()).unwrap();
        assert_eq!(seen.lock().unwrap()[0].0.as_deref(), Some("Resume my change"));
    }

    #[test]
    fn runs_past_the_limit_wait_and_start_in_order() {
        let d = tempfile::tempdir().unwrap();
        let ws = |n: &str| Workspace::create(d.path(), n, "A game", TargetRef::None).unwrap();
        let (a, b, c) = (ws("A"), ws("B"), ws("C"));
        let fake = StandIns::new();
        let hub = hub();
        let mut set = RunSet::new(2).with_starter(fake.starter());
        assert_eq!(set.start(&hub, a.clone(), RunOptions::default()).unwrap(), Started::Running);
        assert_eq!(set.start(&hub, b.clone(), RunOptions::default()).unwrap(), Started::Running);
        assert_eq!(set.start(&hub, c.clone(), RunOptions::default()).unwrap(), Started::Queued { position: 1 });
        assert!(set.start(&hub, c.clone(), RunOptions::default()).is_err(), "a queued project isn't queued twice");
        assert!(set.start(&hub, a.clone(), RunOptions::default()).is_err(), "nor a working one started twice");
        assert_eq!((set.active(), set.position(&c.root), set.busy().len()), (2, Some(1), 3));

        // A finishes: its event is reported under its folder, and C takes its place.
        fake.finish(&a.root);
        let events = set.poll(&hub);
        assert_eq!(events, [(a.root.clone(), RunEvent::Finished { ok: true, message: "Done.".into(), paused: false })]);
        assert_eq!(fake.started(), [a.root.clone(), b.root.clone(), c.root.clone()]);
        assert!(set.is_running(&c.root) && !set.is_running(&a.root) && set.get(&a.root).is_some(), "a finished run is kept");
        // A can build again, and waits behind the two working.
        assert_eq!(set.start(&hub, a.clone(), RunOptions::default()).unwrap(), Started::Queued { position: 1 });
        assert_eq!(set.stop(&a.root), Stopped::Unqueued);
        assert_eq!(set.stop(&b.root), Stopped::Stopping);
        assert_eq!(set.stop(&a.root), Stopped::Nothing);
        set.stop_all();
        assert!(set.position(&a.root).is_none());
    }

    /// An agent app, which has a tool server of its own, and an API backend, which uses the session.
    fn two_backends() -> AiHub {
        let mut s = craft_anything_engine::testing::session();
        s.settings.ai = toml::from_str(
            "[[backends]]\nname = \"Agent\"\nkind = \"agent_cli\"\nmodel = \"claude-code\"\n[[backends]]\nname = \"Key\"\nkind = \"openrouter\"\nmodel = \"m\"\n",
        )
        .unwrap();
        AiHub::new(Arc::new(Engine::new(s)))
    }

    /// #923: two builds in one session wrote into one project, and spent its asset budget.
    #[test]
    fn a_run_in_the_apps_session_works_alone_and_nothing_overtakes_it() {
        let d = tempfile::tempdir().unwrap();
        let ws = |n: &str| Workspace::create(d.path(), n, "A game", TargetRef::None).unwrap();
        let (agent, key, later) = (ws("Agent"), ws("Key"), ws("Later"));
        let hub = two_backends();
        let by = |name: &str| RunOptions { backend: Some(name.into()), ..RunOptions::default() };
        let fake = StandIns::new();
        let mut set = RunSet::new(4).with_starter(fake.starter());

        assert_eq!(set.start(&hub, agent.clone(), by("Agent")).unwrap(), Started::Running);
        assert_eq!(set.start(&hub, key.clone(), by("Key")).unwrap(), Started::Queued { position: 1 }, "it waits for the session");
        assert_eq!(set.start(&hub, later.clone(), by("Agent")).unwrap(), Started::Queued { position: 2 }, "nothing overtakes it");
        assert!(set.poll(&hub).is_empty() && set.active() == 1);

        fake.finish(&agent.root);
        set.poll(&hub);
        assert!(set.is_running(&key.root) && !set.is_running(&later.root), "it has the session to itself, with places free");

        fake.finish(&key.root);
        set.poll(&hub);
        assert!(set.is_running(&later.root));
        assert_eq!(set.start(&hub, agent.clone(), by("Agent")).unwrap(), Started::Running, "agent apps still work side by side");
        set.stop_all();
    }

    /// The marker follows the run, not the project: the same folder rebuilt with an agent app
    /// doesn't keep others waiting.
    #[test]
    fn a_project_that_worked_alone_can_build_beside_others_with_an_agent_app() {
        let d = tempfile::tempdir().unwrap();
        let ws = |n: &str| Workspace::create(d.path(), n, "A game", TargetRef::None).unwrap();
        let (a, b) = (ws("A"), ws("B"));
        let hub = two_backends();
        let by = |name: &str| RunOptions { backend: Some(name.into()), ..RunOptions::default() };
        let fake = StandIns::new();
        let mut set = RunSet::new(2).with_starter(fake.starter());
        assert_eq!(set.start(&hub, a.clone(), by("Key")).unwrap(), Started::Running);
        fake.finish(&a.root);
        set.poll(&hub);
        assert_eq!(set.start(&hub, a.clone(), by("Agent")).unwrap(), Started::Running);
        assert_eq!(set.start(&hub, b.clone(), by("Agent")).unwrap(), Started::Running);
        set.stop_all();
    }

    /// Queued for an agent app, then the only AI left is an API backend: it mustn't start beside
    /// the run still working.
    #[test]
    fn a_queued_run_is_checked_again_when_its_turn_comes() {
        let d = tempfile::tempdir().unwrap();
        let ws = |n: &str| Workspace::create(d.path(), n, "A game", TargetRef::None).unwrap();
        let (a, b, c) = (ws("A"), ws("B"), ws("C"));
        let hub = two_backends();
        let by = |name: &str| RunOptions { backend: Some(name.into()), ..RunOptions::default() };
        let fake = StandIns::new();
        let mut set = RunSet::new(2).with_starter(fake.starter());
        set.start(&hub, a.clone(), by("Agent")).unwrap();
        set.start(&hub, b.clone(), by("Agent")).unwrap();
        assert_eq!(set.start(&hub, c.clone(), RunOptions::default()).unwrap(), Started::Queued { position: 1 });
        hub.engine.session().settings.ai = toml::from_str("[[backends]]\nname = \"Key\"\nkind = \"openrouter\"\nmodel = \"m\"\n").unwrap();

        fake.finish(&a.root);
        set.poll(&hub);
        assert!(!set.is_running(&c.root), "B is still working");
        fake.finish(&b.root);
        set.poll(&hub);
        assert!(set.is_running(&c.root));
        set.stop_all();
    }

    /// The queue reads the project when its turn comes: a Builder saved on it while it waited
    /// decides whether it must work alone, as it decides what the run uses.
    #[test]
    fn a_queued_run_is_judged_by_the_project_on_disk() {
        let d = tempfile::tempdir().unwrap();
        let ws = |n: &str| Workspace::create(d.path(), n, "A game", TargetRef::None).unwrap();
        let (a, b, c) = (ws("A"), ws("B"), ws("C"));
        let hub = two_backends();
        let agent = RunOptions { backend: Some("Agent".into()), ..RunOptions::default() };
        let fake = StandIns::new();
        let mut set = RunSet::new(2).with_starter(fake.starter());
        set.start(&hub, a.clone(), agent.clone()).unwrap();
        set.start(&hub, b.clone(), agent).unwrap();
        assert_eq!(set.start(&hub, c.clone(), RunOptions::default()).unwrap(), Started::Queued { position: 1 });
        // The person picks another Builder for it while it waits.
        Workspace::open(&c.root).unwrap().set_person(|m| m.builder = Some("Key".into())).unwrap();

        fake.finish(&a.root);
        set.poll(&hub);
        assert!(!set.is_running(&c.root), "an API builder works alone, and B is still working");
        fake.finish(&b.root);
        set.poll(&hub);
        assert!(set.is_running(&c.root));
        set.stop_all();
    }

    #[test]
    fn a_follow_up_with_an_api_extractor_can_build_beside_others() {
        let d = tempfile::tempdir().unwrap();
        let mut ws = Workspace::create(d.path(), "A", "A game", TargetRef::None).unwrap();
        let mut s = craft_anything_engine::testing::session();
        s.settings.ai = toml::from_str(
            "[roles]\nextractor = \"Key\"\n[[backends]]\nname = \"Agent\"\nkind = \"agent_cli\"\nmodel = \"claude-code\"\n[[backends]]\nname = \"Key\"\nkind = \"openrouter\"\nmodel = \"m\"\n",
        )
        .unwrap();
        let hub = AiHub::new(Arc::new(Engine::new(s)));
        let by_agent = |message: Option<&str>| RunOptions { backend: Some("Agent".into()), message: message.map(str::to_string), ..RunOptions::default() };
        assert!(must_work_alone(&hub, &ws, &by_agent(None)), "a fresh study may use the Extractor");
        assert!(must_work_alone(&hub, &ws, &by_agent(Some("make it blue"))), "with nothing studied yet, so may this");
        std::fs::create_dir_all(ws.capture_dir()).unwrap();
        ws.manifest.capture_deep = Some(true);
        assert!(!must_work_alone(&hub, &ws, &by_agent(Some("make it blue"))), "a follow-up doesn't");
    }

    #[test]
    fn a_queued_run_that_cannot_start_says_why() {
        let d = tempfile::tempdir().unwrap();
        let a = Workspace::create(d.path(), "A", "A game", TargetRef::None).unwrap();
        let b = Workspace::create(d.path(), "B", "A game", TargetRef::None).unwrap();
        let fake = StandIns::new().failing("B");
        let hub = hub();
        let mut set = RunSet::new(1).with_starter(fake.starter());
        set.start(&hub, a.clone(), RunOptions::default()).unwrap();
        assert_eq!(set.start(&hub, b.clone(), RunOptions::default()).unwrap(), Started::Queued { position: 1 });
        fake.finish(&a.root);
        let events = set.poll(&hub);
        assert!(events.contains(&(b.root.clone(), RunEvent::Finished { ok: false, message: "No coding agent.".into(), paused: false })), "{events:?}");
        assert!(!set.any_busy());
        assert_eq!(Workspace::open(&b.root).unwrap().manifest.status, Status::NeedsYou);
    }

    #[test]
    fn a_queued_run_starts_without_anyone_polling() {
        let d = tempfile::tempdir().unwrap();
        let a = Workspace::create(d.path(), "A", "A game", TargetRef::None).unwrap();
        let b = Workspace::create(d.path(), "B", "A game", TargetRef::None).unwrap();
        let fake = StandIns::new();
        let hub = hub();
        let settings_path = hub.engine.session().config_dir().join("config.toml");
        let set = Arc::new(Mutex::new(RunSet::new(1).with_starter(fake.starter())));
        set.lock().unwrap().start(&hub, a.clone(), RunOptions::default()).unwrap();
        // Nothing waits yet, so no thread is started.
        keep_queue_moving(&set, &hub.engine, settings_path.clone(), Duration::from_millis(5));
        assert!(!set.lock().unwrap().watched);
        set.lock().unwrap().start(&hub, b.clone(), RunOptions::default()).unwrap();
        keep_queue_moving(&set, &hub.engine, settings_path.clone(), Duration::from_millis(5));
        keep_queue_moving(&set, &hub.engine, settings_path, Duration::from_millis(5));
        assert!(set.lock().unwrap().watched);
        fake.finish(&a.root);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while set.lock().unwrap().watched && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(fake.started(), [a.root.clone(), b.root.clone()], "one thread started B once");
        let s = set.lock().unwrap();
        assert!(s.is_running(&b.root) && s.waiting() == 0 && !s.watched);
    }

    #[test]
    fn nothing_is_queued_without_a_coding_agent() {
        let d = tempfile::tempdir().unwrap();
        let a = Workspace::create(d.path(), "A", "A game", TargetRef::None).unwrap();
        let b = Workspace::create(d.path(), "B", "A game", TargetRef::None).unwrap();
        let hub = AiHub::new(Arc::new(Engine::new(craft_anything_engine::testing::session())));
        let mut set = RunSet::new(1).with_starter(StandIns::new().starter());
        set.start(&hub, a, RunOptions::default()).unwrap();
        let root = b.root.clone();
        assert!(set.start(&hub, b, RunOptions::default()).is_err());
        assert_eq!(set.position(&root), None);
    }
}
