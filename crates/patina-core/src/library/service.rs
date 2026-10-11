//! The library thread: scans the configured folders on startup, then
//! watches them with inotify and rescans whatever changes. Rescanning a
//! big library on every launch would be slow and pointless, so startup
//! only reads tags for files whose size or modification time changed.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, select, unbounded};
use notify::{RecursiveMode, Watcher};

use super::scan::{self, ScanStats};
use super::{Library, Result};

#[derive(Debug, Clone, PartialEq)]
pub struct ScanStatus {
    pub root: PathBuf,
    pub done: usize,
    pub total: usize,
}

#[derive(Debug, Clone)]
pub enum LibEvent {
    /// A scan is running. `total` is files whose tags need reading.
    Progress(ScanStatus),
    /// The index changed. Views should re-query.
    Changed(ScanStats),
    /// Startup scan of every root finished.
    Ready,
    Error(String),
}

#[derive(Debug, Clone, Default)]
pub struct ServiceStatus {
    pub scanning: Option<ScanStatus>,
    /// Bumped whenever the index changes.
    pub version: u64,
    pub ready: bool,
    pub watching: bool,
    pub last_error: Option<String>,
}

enum Cmd {
    SetRoots { roots: Vec<PathBuf>, watch: bool },
    Rescan,
    Quit,
}

pub struct LibraryService {
    tx: Sender<Cmd>,
    events: Receiver<LibEvent>,
    status: Arc<Mutex<ServiceStatus>>,
    cancel: Arc<AtomicBool>,
    db_path: PathBuf,
}

/// How long the folders have to stay quiet before a change is scanned.
/// Copying an album fires hundreds of events; this turns them into one scan.
const DEBOUNCE: Duration = Duration::from_millis(1500);

impl LibraryService {
    pub fn start(db_path: &Path, roots: Vec<PathBuf>, watch: bool) -> Result<Self> {
        // Open once here so a broken database fails loudly at startup
        // instead of inside the thread.
        let lib = Library::open(db_path)?;
        let (tx, rx) = unbounded();
        let (ev_tx, ev_rx) = unbounded();
        let status = Arc::new(Mutex::new(ServiceStatus::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        let (st2, c2) = (status.clone(), cancel.clone());
        std::thread::Builder::new()
            .name("patina-library".into())
            .spawn(move || Worker::new(lib, rx, ev_tx, st2, c2).run())
            .expect("spawn library thread");
        let _ = tx.send(Cmd::SetRoots { roots, watch });
        Ok(Self {
            tx,
            events: ev_rx,
            status,
            cancel,
            db_path: db_path.to_path_buf(),
        })
    }

    pub fn events(&self) -> &Receiver<LibEvent> {
        &self.events
    }

    pub fn status(&self) -> ServiceStatus {
        self.status.lock().unwrap().clone()
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn set_roots(&self, roots: Vec<PathBuf>, watch: bool) {
        let _ = self.tx.send(Cmd::SetRoots { roots, watch });
    }

    /// Check every file again (still skips unchanged ones).
    pub fn rescan(&self) {
        let _ = self.tx.send(Cmd::Rescan);
    }
}

impl Drop for LibraryService {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        let _ = self.tx.send(Cmd::Quit);
    }
}

struct Worker {
    lib: Library,
    cmds: Receiver<Cmd>,
    events: Sender<LibEvent>,
    status: Arc<Mutex<ServiceStatus>>,
    cancel: Arc<AtomicBool>,
    roots: Vec<PathBuf>,
    watcher: Option<notify::RecommendedWatcher>,
    fs_tx: Sender<notify::Result<notify::Event>>,
    fs_rx: Receiver<notify::Result<notify::Event>>,
    pending: BTreeSet<PathBuf>,
    last_fs_event: Option<Instant>,
}

impl Worker {
    fn new(
        lib: Library,
        cmds: Receiver<Cmd>,
        events: Sender<LibEvent>,
        status: Arc<Mutex<ServiceStatus>>,
        cancel: Arc<AtomicBool>,
    ) -> Self {
        let (fs_tx, fs_rx) = unbounded();
        Self {
            lib,
            cmds,
            events,
            status,
            cancel,
            roots: Vec::new(),
            watcher: None,
            fs_tx,
            fs_rx,
            pending: BTreeSet::new(),
            last_fs_event: None,
        }
    }

    fn run(mut self) {
        loop {
            let timeout = match self.last_fs_event {
                Some(t) => DEBOUNCE.saturating_sub(t.elapsed()),
                None => Duration::from_secs(3600),
            };
            select! {
                recv(self.cmds) -> cmd => match cmd {
                    Ok(Cmd::Quit) | Err(_) => return,
                    Ok(Cmd::SetRoots { roots, watch }) => self.set_roots(roots, watch),
                    Ok(Cmd::Rescan) => self.full_scan(),
                },
                recv(self.fs_rx) -> ev => {
                    if let Ok(ev) = ev { self.fs_event(ev) }
                },
                default(timeout) => {}
            }
            if self.last_fs_event.is_some_and(|t| t.elapsed() >= DEBOUNCE) {
                self.last_fs_event = None;
                self.scan_pending();
            }
            if self.cancel.load(Ordering::Relaxed) {
                return;
            }
        }
    }

    fn error(&self, msg: String) {
        log::warn!("library: {msg}");
        self.status.lock().unwrap().last_error = Some(msg.clone());
        let _ = self.events.send(LibEvent::Error(msg));
    }

    fn changed(&self, stats: ScanStats) {
        self.status.lock().unwrap().version += 1;
        let _ = self.events.send(LibEvent::Changed(stats));
    }

    fn set_roots(&mut self, roots: Vec<PathBuf>, watch: bool) {
        // Canonical paths, so a root given as ~/Music/ and one given
        // through a symlink still match what inotify reports.
        let roots: Vec<PathBuf> = roots
            .into_iter()
            .map(|r| std::fs::canonicalize(&r).unwrap_or(r))
            .collect();
        self.roots = roots.clone();
        match scan::sync_sources(&mut self.lib.conn, &roots) {
            Ok(_) => {}
            Err(e) => return self.error(e.to_string()),
        }
        self.changed(ScanStats::default());

        // Watch before scanning, so nothing that changes during a long
        // first scan is missed.
        self.watcher = None;
        self.status.lock().unwrap().watching = false;
        if watch && !roots.is_empty() {
            let tx = self.fs_tx.clone();
            match notify::recommended_watcher(move |ev| {
                let _ = tx.send(ev);
            }) {
                Ok(mut w) => {
                    let mut ok = true;
                    for r in &roots {
                        if !r.is_dir() {
                            continue;
                        }
                        if let Err(e) = w.watch(r, RecursiveMode::Recursive) {
                            ok = false;
                            self.error(format!(
                                "can't watch {} ({e}). If this is a large library, raise \
                                 fs.inotify.max_user_watches.",
                                r.display()
                            ));
                        }
                    }
                    self.status.lock().unwrap().watching = ok;
                    self.watcher = Some(w);
                }
                Err(e) => self.error(format!("can't start the folder watcher: {e}")),
            }
        }
        self.full_scan();
    }

    fn full_scan(&mut self) {
        let sources = match self.lib.sources() {
            Ok(s) => s,
            Err(e) => return self.error(e.to_string()),
        };
        let mut total = ScanStats::default();
        for s in sources {
            if self.cancel.load(Ordering::Relaxed) {
                return;
            }
            if !s.root.exists() {
                // A disconnected drive shouldn't wipe its part of the
                // library. Keep the rows; they'll match again on remount.
                self.error(format!(
                    "{} isn't there right now, keeping its tracks",
                    s.root.display()
                ));
                continue;
            }
            match self.scan_one(s.id, &s.root.clone(), &s.root) {
                Ok(st) => total.add(st),
                Err(e) => self.error(e.to_string()),
            }
        }
        {
            let mut st = self.status.lock().unwrap();
            st.scanning = None;
            st.ready = true;
        }
        if total.changed() {
            self.changed(total);
        }
        let _ = self.events.send(LibEvent::Ready);
    }

    fn scan_one(&mut self, source_id: i64, root: &Path, under: &Path) -> Result<ScanStats> {
        let status = self.status.clone();
        let events = self.events.clone();
        let root_buf = root.to_path_buf();
        let progress = move |done: usize, total: usize| {
            let s = ScanStatus {
                root: root_buf.clone(),
                done,
                total,
            };
            let mut st = status.lock().unwrap();
            if st.scanning.as_ref() != Some(&s) {
                st.scanning = Some(s.clone());
                drop(st);
                if total > 0 {
                    let _ = events.send(LibEvent::Progress(s));
                }
            }
        };
        let stats = scan::scan_tree(
            &mut self.lib.conn,
            source_id,
            under,
            &progress,
            &self.cancel,
        )?;
        if stats.unreadable > 0 {
            log::info!(
                "{} files under {} couldn't be read",
                stats.unreadable,
                under.display()
            );
        }
        Ok(stats)
    }

    fn fs_event(&mut self, ev: notify::Result<notify::Event>) {
        use notify::EventKind;
        let ev = match ev {
            Ok(e) => e,
            Err(e) => return self.error(format!("folder watcher: {e}")),
        };
        // Reads and opens change nothing in the index.
        if matches!(ev.kind, EventKind::Access(_)) {
            return;
        }
        for p in &ev.paths {
            self.pending.insert(p.clone());
        }
        // A watcher overflow (too many events at once) means paths were
        // lost; fall back to rescanning every root.
        if ev.need_rescan() {
            for r in self.roots.clone() {
                self.pending.insert(r);
            }
        }
        self.last_fs_event = Some(Instant::now());
    }

    fn scan_pending(&mut self) {
        let pending = std::mem::take(&mut self.pending);
        // Drop paths already covered by an ancestor in the same batch.
        let mut paths: Vec<PathBuf> = Vec::new();
        for p in pending {
            if !paths.iter().any(|a| p.starts_with(a)) {
                paths.push(p);
            }
        }
        let mut total = ScanStats::default();
        for p in paths {
            if !source_root_present(&self.roots, &p) {
                // The drive under this path is gone. Same rule as startup:
                // keep the tracks until it comes back.
                continue;
            }
            let source = match scan::source_for(&self.lib.conn, &p) {
                Ok(Some(s)) => s,
                Ok(None) => continue,
                Err(e) => {
                    self.error(e.to_string());
                    continue;
                }
            };
            // A file event for something that isn't audio and isn't in the
            // index (cover.jpg, a .part download) needs no scan.
            if p.is_file()
                && !crate::track::is_audio_file(&p)
                && !scan::is_indexed(&self.lib.conn, &p).unwrap_or(false)
            {
                continue;
            }
            match self.scan_one(source.0, &source.1, &p) {
                Ok(st) => total.add(st),
                Err(e) => self.error(e.to_string()),
            }
        }
        self.status.lock().unwrap().scanning = None;
        if total.changed() {
            self.changed(total);
        }
    }
}

/// True when the root containing `p` still exists.
fn source_root_present(roots: &[PathBuf], p: &Path) -> bool {
    roots
        .iter()
        .filter(|r| p.starts_with(r))
        .max_by_key(|r| r.as_os_str().len())
        .is_some_and(|r| r.is_dir())
}
