//! Shared test harness: an engine on temporary folders with a folder Trash.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, unbounded};
use fstudio_core::*;

pub struct H {
    pub dir: tempfile::TempDir,
    pub root: PathBuf,
    pub data: PathBuf,
    pub trash: PathBuf,
    pub engine: Arc<Engine>,
    pub rx: Receiver<CoreEvent>,
}

pub const T: Duration = Duration::from_secs(10);

pub fn config(dir: &Path) -> EngineConfig {
    EngineConfig {
        data_dir: dir.join("data").to_string_lossy().into_owned(),
        home: Some(dir.join("root").to_string_lossy().into_owned()),
        trash_dir: Some(dir.join("trash").to_string_lossy().into_owned()),
        poll_watcher: false,
    }
}

pub fn harness() -> H {
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::create_dir_all(base.join("root")).unwrap();
    std::fs::create_dir_all(base.join("trash")).unwrap();
    let (tx, rx) = unbounded();
    let engine = Engine::new(config(&base), Arc::new(tx)).unwrap();
    // Temporary folders live under /private/var/folders, which frecency skips by default.
    engine
        .set_setting("path_bar.frecency_excluded", "[]")
        .unwrap();
    H {
        root: base.join("root"),
        data: base.join("data"),
        trash: base.join("trash"),
        dir,
        engine,
        rx,
    }
}

impl H {
    /// Reopens the engine on the same folders (a relaunch).
    pub fn relaunch(&mut self) {
        self.engine.app_will_quit();
        let base = self.root.parent().unwrap().to_path_buf();
        let (tx, rx) = unbounded();
        self.engine = Engine::new(config(&base), Arc::new(tx)).unwrap();
        self.rx = rx;
    }

    pub fn p(&self, rel: &str) -> PathBuf {
        if rel.is_empty() {
            self.root.clone()
        } else {
            self.root.join(rel)
        }
    }

    pub fn s(&self, rel: &str) -> String {
        self.p(rel).to_string_lossy().into_owned()
    }

    /// One window at `rel`, with its tab's listing complete.
    pub fn window_at(&self, rel: &str) -> Arc<Tab> {
        self.engine.new_window(Some(self.s(rel))).unwrap();
        let t = self.engine.active_tab().unwrap();
        wait_complete(&t);
        t
    }

    /// Drains events until one matches, or panics after `T`.
    pub fn expect_event(&self, what: &str, f: impl Fn(&CoreEvent) -> bool) -> CoreEvent {
        let end = Instant::now() + T;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left) {
                Ok(e) if f(&e) => return e,
                Ok(_) => {}
                Err(_) => panic!("timed out waiting for {what}"),
            }
        }
    }

    pub fn drain(&self) {
        self.engine.flush_events();
        while self.rx.try_recv().is_ok() {}
    }
}

pub fn eventually(what: &str, f: impl Fn() -> bool) {
    let end = Instant::now() + T;
    while Instant::now() < end {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out waiting for {what}");
}

pub fn wait_complete(t: &Tab) {
    eventually("listing to complete", || {
        t.listing().state() != ListingState::Loading
    });
}

pub fn names(t: &Tab) -> Vec<String> {
    t.listing().names()
}

pub fn touch(p: &Path) {
    std::fs::write(p, b"x").unwrap();
}

pub fn mkdirs(p: &Path) {
    std::fs::create_dir_all(p).unwrap();
}
