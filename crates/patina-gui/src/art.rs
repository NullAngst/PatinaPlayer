//! Cover art for the GUI, loaded on worker threads and handed to Slint on
//! the UI thread. Images are cached by source and size, with a cap so a big
//! album grid doesn't hold every cover in memory forever.
//!
//! Requests are taken newest first: scrolling quickly through thousands of
//! albums asks for every cover it passes, and the ones on screen when it
//! stops should load first, not after all the ones that went by. Requests
//! that pile up past a limit are dropped, oldest first, and asked for
//! again if their rows come back on screen.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use crossbeam_channel::{Receiver, unbounded};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

/// What to find art for.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Req {
    /// A track: its embedded picture, else a cover in its folder.
    Track(PathBuf),
    /// A folder: a cover image inside it.
    Dir(PathBuf),
    /// An artist's picture, by name: from their folder or a server.
    Artist(String),
}

impl Req {
    /// Whether a cover saved for the album in `dir` is what this shows.
    fn under(&self, dir: &Path) -> bool {
        match self {
            Req::Track(p) => p.parent().is_some_and(|d| {
                d.starts_with(dir) || patina_core::library::tags::album_dir(d) == dir
            }),
            Req::Dir(d) => d.starts_with(dir) || patina_core::library::tags::album_dir(d) == dir,
            Req::Artist(_) => false,
        }
    }
}

/// Grid tiles and list thumbnails.
pub const THUMB: u32 = 192;
/// The album header and the bottom bar.
pub const LARGE: u32 = 400;

/// Most decoded images kept at once.
const CAP: usize = 900;
/// Most requests waiting at once; older ones beyond it are dropped.
const WAITING: usize = 300;

type Key = (Req, u32);

type Done = (Key, Option<SharedPixelBuffer<Rgba8Pixel>>);

/// Requests waiting, newest at the back.
struct Waiting {
    keys: Mutex<VecDeque<Key>>,
    ready: Condvar,
}

thread_local! {
    /// Each worker's own connection to the library, for finding an
    /// artist's folder and servers.
    static LIB: std::cell::RefCell<Option<patina_core::library::Library>> =
        const { std::cell::RefCell::new(None) };
}

fn with_library<T>(f: impl FnOnce(&patina_core::library::Library) -> Option<T>) -> Option<T> {
    LIB.with(|l| {
        let mut l = l.borrow_mut();
        if l.is_none() {
            *l = patina_core::library::Library::open(&patina_core::config::library_db_path()).ok();
        }
        f(l.as_ref()?)
    })
}

pub struct ArtCache {
    images: HashMap<Key, Option<Image>>,
    order: VecDeque<Key>,
    pending: HashSet<Key>,
    waiting: Arc<Waiting>,
    rx: Receiver<Done>,
}

impl ArtCache {
    pub fn new() -> Self {
        let waiting = Arc::new(Waiting {
            keys: Mutex::new(VecDeque::new()),
            ready: Condvar::new(),
        });
        let (done_tx, rx) = unbounded::<Done>();
        // Two workers: enough to keep up with scrolling, few enough that a
        // first visit to a huge album grid doesn't hog the disk.
        for n in 0..2 {
            let waiting = waiting.clone();
            let done = done_tx.clone();
            std::thread::Builder::new()
                .name(format!("patina-art-{n}"))
                .spawn(move || {
                    loop {
                        let key = {
                            let mut q = waiting.keys.lock().unwrap();
                            loop {
                                if let Some(k) = q.pop_back() {
                                    break k;
                                }
                                q = waiting.ready.wait(q).unwrap();
                            }
                        };
                        let img = match &key.0 {
                            Req::Track(p) => patina_core::art::load(p, key.1),
                            Req::Dir(d) => patina_core::art::load_dir(d, key.1),
                            Req::Artist(name) => {
                                with_library(|lib| patina_core::artist_art::load(lib, name, key.1))
                            }
                        };
                        let buf = img.map(|i| {
                            SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                                i.as_raw(),
                                i.width(),
                                i.height(),
                            )
                        });
                        if done.send((key, buf)).is_err() {
                            return;
                        }
                    }
                })
                .expect("spawn art worker");
        }
        Self {
            images: HashMap::new(),
            order: VecDeque::new(),
            pending: HashSet::new(),
            waiting,
            rx,
        }
    }

    /// The image if it's ready. Queues a load if it isn't known yet.
    pub fn get(&mut self, req: Req, size: u32) -> Option<Image> {
        let key = (req, size);
        if let Some(img) = self.images.get(&key) {
            return img.clone();
        }
        if self.pending.insert(key.clone()) {
            let mut q = self.waiting.keys.lock().unwrap();
            q.push_back(key);
            while q.len() > WAITING {
                if let Some(old) = q.pop_front() {
                    // Not loading after all: asked for again when it shows.
                    self.pending.remove(&old);
                }
            }
            drop(q);
            self.waiting.ready.notify_one();
        }
        None
    }

    /// Asked for and still loading, so a placeholder can shimmer
    /// instead of showing "no cover".
    pub fn loading(&self, req: &Req, size: u32) -> bool {
        self.pending.contains(&(req.clone(), size))
    }

    /// Take finished loads. True if anything new arrived.
    pub fn collect(&mut self) -> bool {
        let mut any = false;
        while let Ok((key, buf)) = self.rx.try_recv() {
            // Forgotten while it loaded (a new cover chosen): asked again.
            if !self.pending.remove(&key) {
                continue;
            }
            self.images.insert(key.clone(), buf.map(Image::from_rgba8));
            self.order.push_back(key);
            any = true;
        }
        while self.order.len() > CAP {
            if let Some(old) = self.order.pop_front() {
                self.images.remove(&old);
            }
        }
        any
    }

    /// A new cover was chosen for the album in `dir`: forget what was
    /// loaded for it, so it's loaded again.
    pub fn forget_under(&mut self, dir: &Path) {
        self.images.retain(|k, _| !k.0.under(dir));
        self.order.retain(|k| !k.0.under(dir));
        self.pending.retain(|k| !k.0.under(dir));
    }
}
