use crate::app::AppCore;
use crate::db::Db;
use crate::events::AppEvent;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;

const LRU_CAPACITY: usize = 300;

struct DecodeRequest {
    key: String,
    title: String,
    artist: String,
    size: u32,
    generation: u64,
}

struct DecodeResult {
    key: String,
    title: String,
    artist: String,
    size: u32,
    generation: u64,
    width: u32,
    height: u32,
    rgba: Option<Vec<u8>>,
    dominant: Option<(u8, u8, u8)>,
}

type Callback = Box<dyn Fn(Option<&gdk::MemoryTexture>, Option<(u8, u8, u8)>)>;

pub struct ArtworkCache {
    db_path: PathBuf,
    textures: RefCell<HashMap<String, (gdk::MemoryTexture, Option<(u8, u8, u8)>)>>,
    lru: RefCell<VecDeque<String>>,
    misses: RefCell<HashSet<String>>,
    pending: RefCell<HashMap<String, Vec<Callback>>>,
    generations: RefCell<HashMap<String, u64>>,
    placeholders: RefCell<HashMap<u64, gdk::MemoryTexture>>,
    tx: RefCell<Option<async_channel::Sender<DecodeRequest>>>,
}

impl ArtworkCache {
    pub fn new(db_path: PathBuf) -> Self {
        Self {
            db_path,
            textures: RefCell::new(HashMap::new()),
            lru: RefCell::new(VecDeque::new()),
            misses: RefCell::new(HashSet::new()),
            pending: RefCell::new(HashMap::new()),
            generations: RefCell::new(HashMap::new()),
            placeholders: RefCell::new(HashMap::new()),
            tx: RefCell::new(None),
        }
    }

    pub fn start(&self, core: &Rc<AppCore>) {
        let (req_tx, req_rx) = async_channel::unbounded::<DecodeRequest>();
        let (res_tx, res_rx) = async_channel::unbounded::<DecodeResult>();
        let db_path = self.db_path.clone();
        std::thread::Builder::new()
            .name("flaccy-artwork".into())
            .spawn(move || decode_worker(db_path, req_rx, res_tx))
            .ok();
        *self.tx.borrow_mut() = Some(req_tx);

        let core = Rc::downgrade(core);
        glib::spawn_future_local(async move {
            while let Ok(result) = res_rx.recv().await {
                let Some(core) = core.upgrade() else { break };
                core.artwork.finish(result);
            }
        });
    }

    pub fn placeholder(&self, seed: &str) -> gdk::MemoryTexture {
        let hash = crate::palette::fnv1a_64(seed);
        if let Some(texture) = self.placeholders.borrow().get(&hash) {
            return texture.clone();
        }
        let size = 160u32;
        let data = crate::palette::placeholder_rgba(seed, size);
        let texture = gdk::MemoryTexture::new(
            size as i32,
            size as i32,
            gdk::MemoryFormat::R8g8b8a8,
            &glib::Bytes::from_owned(data),
            (size * 4) as usize,
        );
        self.placeholders.borrow_mut().insert(hash, texture.clone());
        texture
    }

    /// Drops cached textures and negative-cache entries for one album so a
    /// freshly enriched or chosen cover is re-decoded on the next request, and
    /// bumps the album's generation so a decode already in flight — which may
    /// have read the old cover — is thrown away and redone when it lands.
    pub fn invalidate(&self, title: &str, artist: &str) {
        *self
            .generations
            .borrow_mut()
            .entry(album_id(title, artist))
            .or_insert(0) += 1;
        let prefix = format!("{title}|{artist}|");
        self.textures
            .borrow_mut()
            .retain(|key, _| !key.starts_with(&prefix));
        self.misses
            .borrow_mut()
            .retain(|key| !key.starts_with(&prefix));
        self.lru
            .borrow_mut()
            .retain(|key| !key.starts_with(&prefix));
    }

    fn generation(&self, title: &str, artist: &str) -> u64 {
        self.generations
            .borrow()
            .get(&album_id(title, artist))
            .copied()
            .unwrap_or(0)
    }

    /// Requests decoded album art; the callback fires immediately on cache hit,
    /// or later on the main loop after a worker decode. `None` means the album
    /// has no embedded art and the caller should keep its placeholder.
    pub fn request(
        &self,
        title: &str,
        artist: &str,
        size: u32,
        callback: impl Fn(Option<&gdk::MemoryTexture>, Option<(u8, u8, u8)>) + 'static,
    ) {
        let size = size * display_scale();
        let key = format!("{title}|{artist}|{size}");
        if let Some((texture, dominant)) = self.textures.borrow().get(&key) {
            self.touch(&key);
            callback(Some(texture), *dominant);
            return;
        }
        if self.misses.borrow().contains(&key) {
            callback(None, None);
            return;
        }
        let mut pending = self.pending.borrow_mut();
        let entry = pending.entry(key.clone()).or_default();
        let first = entry.is_empty();
        entry.push(Box::new(callback));
        drop(pending);
        if first {
            if let Some(tx) = self.tx.borrow().as_ref() {
                let _ = tx.send_blocking(DecodeRequest {
                    key,
                    title: title.to_string(),
                    artist: artist.to_string(),
                    size,
                    generation: self.generation(title, artist),
                });
            }
        }
    }

    /// A decode that began before the album was invalidated may carry the old
    /// cover: it is not cached and nothing is told, the request is sent again
    /// and the waiting callbacks stay queued for the fresh answer.
    fn resend_if_stale(&self, result: &DecodeResult) -> bool {
        let current = self.generation(&result.title, &result.artist);
        if result.generation == current {
            return false;
        }
        if let Some(tx) = self.tx.borrow().as_ref() {
            let _ = tx.send_blocking(DecodeRequest {
                key: result.key.clone(),
                title: result.title.clone(),
                artist: result.artist.clone(),
                size: result.size,
                generation: current,
            });
        }
        true
    }

    fn finish(&self, result: DecodeResult) {
        if self.resend_if_stale(&result) {
            return;
        }
        let callbacks = self.pending.borrow_mut().remove(&result.key);
        let texture = result.rgba.map(|rgba| {
            gdk::MemoryTexture::new(
                result.width as i32,
                result.height as i32,
                gdk::MemoryFormat::R8g8b8a8,
                &glib::Bytes::from_owned(rgba),
                (result.width * 4) as usize,
            )
        });
        match &texture {
            Some(texture) => {
                self.textures
                    .borrow_mut()
                    .insert(result.key.clone(), (texture.clone(), result.dominant));
                self.touch(&result.key);
                self.evict_if_needed();
            }
            None => {
                self.misses.borrow_mut().insert(result.key.clone());
            }
        }
        if let Some(callbacks) = callbacks {
            for callback in callbacks {
                callback(texture.as_ref(), result.dominant);
            }
        }
    }

    fn touch(&self, key: &str) {
        let mut lru = self.lru.borrow_mut();
        lru.retain(|k| k != key);
        lru.push_back(key.to_string());
    }

    fn evict_if_needed(&self) {
        let mut lru = self.lru.borrow_mut();
        while lru.len() > LRU_CAPACITY {
            if let Some(oldest) = lru.pop_front() {
                self.textures.borrow_mut().remove(&oldest);
            }
        }
    }
}

/// Shows an album's cover in a picture and keeps it right: a cover chosen or
/// reset later repaints it, falling back to the album's placeholder when the
/// release has no art left at all.
pub fn follow_picture(
    core: &Rc<AppCore>,
    picture: &gtk::Picture,
    title: &str,
    artist: &str,
    size: u32,
) {
    {
        let weak = picture.downgrade();
        core.artwork.request(title, artist, size, move |texture, _| {
            if let (Some(picture), Some(texture)) = (weak.upgrade(), texture) {
                picture.set_paintable(Some(texture));
            }
        });
    }
    let (title, artist) = (title.to_string(), artist.to_string());
    let weak_core = Rc::downgrade(core);
    core.hub.subscribe_widget(picture, move |picture, event| {
        let AppEvent::AlbumArtworkChanged {
            title: changed_title,
            artist: changed_artist,
        } = event
        else {
            return;
        };
        if changed_title != &title || changed_artist != &artist {
            return;
        }
        let Some(core) = weak_core.upgrade() else {
            return;
        };
        let weak = picture.downgrade();
        let placeholder = core.artwork.placeholder(&format!("{title}|{artist}"));
        core.artwork.request(&title, &artist, size, move |texture, _| {
            if let Some(picture) = weak.upgrade() {
                match texture {
                    Some(texture) => picture.set_paintable(Some(texture)),
                    None => picture.set_paintable(Some(&placeholder)),
                }
            }
        });
    });
}

fn album_id(title: &str, artist: &str) -> String {
    format!("{title}|{artist}")
}

/// Highest scale factor across connected monitors, so decoded textures stay
/// crisp on HiDPI displays (GTK downscales the paintable to logical size).
fn display_scale() -> u32 {
    use gtk::prelude::*;
    let Some(display) = gdk::Display::default() else {
        return 1;
    };
    let monitors = display.monitors();
    let mut scale = 1;
    for index in 0..monitors.n_items() {
        if let Some(monitor) = monitors
            .item(index)
            .and_then(|obj| obj.downcast::<gdk::Monitor>().ok())
        {
            scale = scale.max(monitor.scale_factor() as u32);
        }
    }
    scale.max(1)
}

fn decode_worker(
    db_path: PathBuf,
    rx: async_channel::Receiver<DecodeRequest>,
    tx: async_channel::Sender<DecodeResult>,
) {
    let db = match Db::open(&db_path) {
        Ok(db) => db,
        Err(err) => {
            crate::logger::error("ui", &format!("artwork worker db open failed: {err}"));
            return;
        }
    };
    while let Ok(request) = rx.recv_blocking() {
        let result = decode_one(&db, &request);
        if tx.send_blocking(result).is_err() {
            break;
        }
    }
}

fn decode_one(db: &Db, request: &DecodeRequest) -> DecodeResult {
    let empty = DecodeResult {
        key: request.key.clone(),
        title: request.title.clone(),
        artist: request.artist.clone(),
        size: request.size,
        generation: request.generation,
        width: 0,
        height: 0,
        rgba: None,
        dominant: None,
    };
    let Some(data) = db.fetch_album_artwork(&request.title, &request.artist) else {
        return empty;
    };
    let Ok(decoded) = image::load_from_memory(&data) else {
        return empty;
    };
    let thumbnail = decoded.thumbnail(request.size, request.size).to_rgba8();
    let (width, height) = thumbnail.dimensions();
    let rgba = thumbnail.into_raw();
    let dominant = Some(crate::palette::dominant_color(&rgba, width, height));
    DecodeResult {
        key: request.key.clone(),
        title: request.title.clone(),
        artist: request.artist.clone(),
        size: request.size,
        generation: request.generation,
        width,
        height,
        rgba: Some(rgba),
        dominant,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache_with_channel() -> (ArtworkCache, async_channel::Receiver<DecodeRequest>) {
        let cache = ArtworkCache::new(PathBuf::from("unused.sqlite"));
        let (tx, rx) = async_channel::unbounded();
        *cache.tx.borrow_mut() = Some(tx);
        (cache, rx)
    }

    fn result(generation: u64) -> DecodeResult {
        DecodeResult {
            key: "Record|Band|168".into(),
            title: "Record".into(),
            artist: "Band".into(),
            size: 168,
            generation,
            width: 0,
            height: 0,
            rgba: None,
            dominant: None,
        }
    }

    #[test]
    fn a_decode_that_started_before_an_invalidation_is_redone_not_trusted() {
        let (cache, rx) = cache_with_channel();
        assert!(!cache.resend_if_stale(&result(0)), "nothing changed, the answer stands");
        assert!(rx.try_recv().is_err());

        cache.invalidate("Record", "Band");
        assert_eq!(cache.generation("Record", "Band"), 1);
        assert!(cache.resend_if_stale(&result(0)), "the old read is stale");
        let again = rx.try_recv().expect("the request is sent again");
        assert_eq!(again.generation, 1, "carrying the new generation");
        assert_eq!((again.title.as_str(), again.size), ("Record", 168));
        assert!(!cache.resend_if_stale(&result(1)), "the redo is current");
    }

    #[test]
    fn invalidating_one_album_leaves_another_alone() {
        let (cache, _rx) = cache_with_channel();
        cache.invalidate("Record", "Band");
        assert_eq!(cache.generation("Other", "Band"), 0);
        cache.invalidate("Record", "Band");
        assert_eq!(cache.generation("Record", "Band"), 2);
    }

    #[test]
    fn invalidating_clears_the_negative_cache_for_every_size() {
        let (cache, _rx) = cache_with_channel();
        cache.misses.borrow_mut().insert("Record|Band|168".into());
        cache.misses.borrow_mut().insert("Record|Band|464".into());
        cache.misses.borrow_mut().insert("Other|Band|168".into());
        cache.invalidate("Record", "Band");
        let remaining: Vec<String> = cache.misses.borrow().iter().cloned().collect();
        assert_eq!(remaining, vec!["Other|Band|168".to_string()]);
    }
}
