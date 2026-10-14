//! Cover art for the GUI, loaded on worker threads and handed to Slint on
//! the UI thread. Images are cached by source and size, with a cap so a big
//! album grid doesn't hold every cover in memory forever.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use crossbeam_channel::{Receiver, Sender, unbounded};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

/// What to find art for.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Req {
    /// A track: its embedded picture, else a cover in its folder.
    Track(PathBuf),
    /// A folder: a cover image inside it.
    Dir(PathBuf),
}

/// Grid tiles and list thumbnails.
pub const THUMB: u32 = 192;
/// The album header and the bottom bar.
pub const LARGE: u32 = 400;

/// Most decoded images kept at once.
const CAP: usize = 900;

type Key = (Req, u32);
type Loaded = (Key, Option<SharedPixelBuffer<Rgba8Pixel>>);

pub struct ArtCache {
    images: HashMap<Key, Option<Image>>,
    order: VecDeque<Key>,
    pending: HashSet<Key>,
    tx: Sender<Key>,
    rx: Receiver<Loaded>,
}

impl ArtCache {
    pub fn new() -> Self {
        let (tx, jobs) = unbounded::<Key>();
        let (done_tx, rx) = unbounded::<Loaded>();
        // Two workers: enough to keep up with scrolling, few enough that a
        // first visit to a huge album grid doesn't hog the disk.
        for n in 0..2 {
            let jobs = jobs.clone();
            let done = done_tx.clone();
            std::thread::Builder::new()
                .name(format!("patina-art-{n}"))
                .spawn(move || {
                    for key in jobs {
                        let img = match &key.0 {
                            Req::Track(p) => patina_core::art::load(p, key.1),
                            Req::Dir(d) => patina_core::art::load_dir(d, key.1),
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
            tx,
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
            let _ = self.tx.send(key);
        }
        None
    }

    /// Take finished loads. True if anything new arrived.
    pub fn collect(&mut self) -> bool {
        let mut any = false;
        while let Ok((key, buf)) = self.rx.try_recv() {
            self.pending.remove(&key);
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
}
