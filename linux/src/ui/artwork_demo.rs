//! Demo and verification hooks for the artwork editor, driven from the
//! environment the way the other `FLACCY_DEMO_*` switches are:
//!
//! - `FLACCY_DEMO_ARTWORK='Album|Artist'` opens the editor for that album
//!   (the artist is optional).
//! - `FLACCY_DEMO_ARTWORK_IMAGE=<path>` opens it with that image already
//!   loaded and framed, as a drop on the cover would.
//! - `FLACCY_DEMO_ARTWORK_QUERY=<text>` searches the online shelf for that.
//! - `FLACCY_DEMO_ARTWORK_DRIVE=1` drives pan, zoom, double click, pinch, keys,
//!   Save and Use Original Artwork through the same handlers the real gestures
//!   use, against the real database, and logs `ARTWORK DRIVE PASS` or `FAIL`
//!   for each check and `ARTWORK DRIVE PASSED` or `FAILED` at the end.
//!   `FLACCY_DEMO_ARTWORK_DRIVE=online` also picks the first online cover.

use crate::events::AppEvent;
use crate::library::Album;
use crate::ui::artwork_editor::{self, Editor, Payload};
use crate::ui::Ui;
use image::{Rgba, RgbaImage};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

type Dominant = Option<(u8, u8, u8)>;

const POLL: Duration = Duration::from_millis(50);
const PATIENCE: Duration = Duration::from_secs(12);

pub fn open(ui: &Rc<Ui>, spec: &str) {
    let Some(album) = find_album(ui, spec) else {
        crate::logger::warn("artwork", &format!("demo: no album matches {spec}"));
        return;
    };
    let preload = std::env::var_os("FLACCY_DEMO_ARTWORK_IMAGE")
        .map(std::path::PathBuf::from)
        .map(Payload::Path);
    let drive = std::env::var("FLACCY_DEMO_ARTWORK_DRIVE").ok();
    if drive.is_some() {
        let ui = Rc::clone(ui);
        let online = drive.as_deref() == Some("online");
        gtk::glib::spawn_future_local(async move {
            Drive::new(ui, album, online).run().await;
        });
        return;
    }
    let editor = artwork_editor::present(ui, &album, preload);
    if let Ok(query) = std::env::var("FLACCY_DEMO_ARTWORK_QUERY") {
        editor.search_for(&query);
    }
}

fn find_album(ui: &Rc<Ui>, spec: &str) -> Option<Album> {
    let (title, artist) = spec.split_once('|').unwrap_or((spec, ""));
    ui.core
        .library
        .borrow()
        .albums
        .iter()
        .find(|album| album.title == title && (artist.is_empty() || album.artist == artist))
        .cloned()
}

struct Drive {
    ui: Rc<Ui>,
    album: Album,
    online: bool,
    failures: Cell<usize>,
    announcements: Rc<RefCell<Vec<(String, String)>>>,
}

impl Drive {
    fn new(ui: Rc<Ui>, album: Album, online: bool) -> Self {
        Self {
            ui,
            album,
            online,
            failures: Cell::new(0),
            announcements: Rc::new(RefCell::new(Vec::new())),
        }
    }

    fn check(&self, name: &str, passed: bool, detail: &str) {
        if passed {
            crate::logger::info("artwork", &format!("ARTWORK DRIVE PASS: {name} ({detail})"));
        } else {
            self.failures.set(self.failures.get() + 1);
            crate::logger::error("artwork", &format!("ARTWORK DRIVE FAIL: {name} ({detail})"));
        }
    }

    async fn until(&self, mut condition: impl FnMut() -> bool) -> bool {
        let started = Instant::now();
        while !condition() {
            if started.elapsed() > PATIENCE {
                return false;
            }
            gtk::glib::timeout_future(POLL).await;
        }
        true
    }

    async fn run(self) {
        gtk::glib::timeout_future(Duration::from_millis(1400)).await;
        self.start_from_the_original();
        self.listen();
        let original = self.ui.core.db.fetch_album_artwork(&self.album.title, &self.album.artist);
        let original_color = self.cached_color().await;
        let editor = artwork_editor::present(&self.ui, &self.album, None);
        let loaded = self.until(|| editor.is_loaded()).await;
        self.check("the current artwork loads", loaded, &editor.info_text());
        if loaded {
            if let Ok(query) = std::env::var("FLACCY_DEMO_ARTWORK_QUERY") {
                editor.search_for(&query);
            }
            self.framing_checks(&editor).await;
            self.shelf_checks(&editor).await;
            self.frame_the_red_corner_and_save(&editor).await;
        }
        self.after_save(original, original_color).await;
        self.finish();
    }

    /// A run starts from the files' own artwork, whatever an earlier run or a
    /// person left in the demo library.
    fn start_from_the_original(&self) {
        let keys = self.album.artwork_keys();
        if self.ui.core.db.clear_custom_cover(&keys).is_ok() {
            self.ui.core.album_artwork_changed(&keys);
        }
    }

    fn listen(&self) {
        let seen = Rc::clone(&self.announcements);
        self.ui.core.hub.subscribe(move |event| {
            if let AppEvent::AlbumArtworkChanged { title, artist } = event {
                seen.borrow_mut().push((title.clone(), artist.clone()));
            }
            true
        });
    }

    async fn framing_checks(&self, editor: &Rc<Editor>) {
        let crop = editor.crop();
        let started = crop.framing().expect("framing");
        self.check(
            "a freshly loaded cover is filled and centred with nothing to save",
            crop.is_pristine() && started.is_at_fill() && !editor.save_is_enabled(),
            &format!("zoom {:.3}", started.zoom()),
        );

        let side = crop.side();
        crop.scrolled(-6.0, side / 2.0, side / 2.0);
        let zoomed = crop.framing().expect("framing");
        self.check(
            "the wheel zooms in around the pointer",
            zoomed.zoom() > started.zoom() * 1.5 && !crop.is_pristine(),
            &format!("{:.3} -> {:.3}", started.zoom(), zoomed.zoom()),
        );
        self.check(
            "the zoom slider follows the framing and Save wakes up",
            (editor.zoom_value() - zoomed.zoom_fraction()).abs() < 0.01 && editor.save_is_enabled(),
            &format!("slider {:.3}", editor.zoom_value()),
        );
        self.check(
            "closing now would ask before discarding",
            editor.would_ask_before_closing(),
            "",
        );

        let before = crop.framing().expect("framing");
        crop.drag_begin();
        crop.drag_update(-30.0, -20.0);
        crop.drag_update(-60.0, -40.0);
        crop.drag_end();
        let dragged = crop.framing().expect("framing");
        self.check(
            "dragging moves the cover with the pointer",
            dragged.center_x() > before.center_x() + 1.0
                && dragged.center_y() > before.center_y() + 1.0
                && (dragged.zoom() - before.zoom()).abs() < 1e-9,
            &format!(
                "centre ({:.1}, {:.1}) -> ({:.1}, {:.1})",
                before.center_x(),
                before.center_y(),
                dragged.center_x(),
                dragged.center_y()
            ),
        );

        crop.toggle_zoom(side / 2.0, side / 2.0);
        let toggled = crop.framing().expect("framing");
        self.check(
            "a double click returns a zoomed cover to filled",
            toggled.is_at_fill(),
            "",
        );
        crop.toggle_zoom(side / 2.0, side / 2.0);
        let toggled = crop.framing().expect("framing");
        self.check(
            "a second double click zooms in to two and a half times",
            !toggled.is_at_fill()
                && (toggled.zoom() - (toggled.fill_zoom() * 2.5).min(toggled.max_zoom())).abs() < 1e-6,
            &format!("{:.3}", toggled.zoom() / toggled.fill_zoom()),
        );

        let key_start = crop.framing().expect("framing");
        crop.key_pressed(gtk::gdk::Key::Right, gtk::gdk::ModifierType::empty());
        let nudged = crop.framing().expect("framing");
        crop.key_pressed(gtk::gdk::Key::Right, gtk::gdk::ModifierType::SHIFT_MASK);
        let shoved = crop.framing().expect("framing");
        let small = (nudged.center_x() - key_start.center_x()).abs() * key_start.zoom();
        let large = (shoved.center_x() - nudged.center_x()).abs() * key_start.zoom();
        self.check(
            "arrow keys nudge ten points and Shift forty",
            (small - 10.0).abs() < 0.01 && (large - 40.0).abs() < 0.01,
            &format!("{small:.2} then {large:.2}"),
        );

        let before_zoom = crop.framing().expect("framing").zoom();
        crop.key_pressed(gtk::gdk::Key::plus, gtk::gdk::ModifierType::empty());
        let after_plus = crop.framing().expect("framing").zoom();
        crop.key_pressed(gtk::gdk::Key::minus, gtk::gdk::ModifierType::empty());
        let after_minus = crop.framing().expect("framing").zoom();
        self.check(
            "plus and minus zoom by a tenth",
            (after_plus / before_zoom - 1.1).abs() < 1e-6 && (after_minus - before_zoom).abs() < 1e-6,
            &format!("{:.4} {:.4}", after_plus / before_zoom, after_minus / before_zoom),
        );

        crop.pinch_begin();
        crop.pinch_scale(1.4, (side / 2.0, side / 2.0));
        let pinched = crop.framing().expect("framing");
        self.check(
            "a pinch zooms from where the gesture began",
            (pinched.zoom() / before_zoom - 1.4).abs() < 1e-6
                || (pinched.zoom() - pinched.max_zoom()).abs() < 1e-6,
            &format!("{:.3}", pinched.zoom() / before_zoom),
        );

        editor.set_zoom_value(0.5);
        let from_slider = crop.framing().expect("framing");
        self.check(
            "the slider zooms to its fraction",
            (from_slider.zoom_fraction() - 0.5).abs() < 0.02,
            &format!("{:.3}", from_slider.zoom_fraction()),
        );

        let visible_before = from_slider.visible_side();
        crop.resize(side * 0.75);
        let resized = crop.framing().expect("framing");
        self.check(
            "resizing the window keeps what the frame showed",
            (resized.visible_side() - visible_before).abs() < 1e-6,
            &format!("{visible_before:.1}"),
        );
        crop.resize(side);

        crop.key_pressed(gtk::gdk::Key::_0, gtk::gdk::ModifierType::empty());
        self.check(
            "0 fits the cover and leaves nothing to save",
            crop.is_pristine() && !editor.save_is_enabled() && !editor.would_ask_before_closing(),
            "",
        );
        crop.scrolled(-3.0, 10.0, 10.0);
        crop.fit();
        self.check("Fit resets the framing", crop.is_pristine(), "");
    }

    async fn shelf_checks(&self, editor: &Rc<Editor>) {
        let local = self.until(|| editor.files_tile_count() > 0).await;
        self.check(
            "the In Your Files shelf lists the album's embedded cover",
            local,
            &format!("{} tiles", editor.files_tile_count()),
        );
        let settled = self
            .until(|| editor.online_tile_count() > 0 || editor.online_status_text().is_some())
            .await;
        let detail = format!(
            "{} tiles, status {:?}",
            editor.online_tile_count(),
            editor.online_status_text()
        );
        self.check("the Find Online shelf answers", settled, &detail);
        let clickable = self.online && self.until(|| editor.click_first_online_tile()).await;
        if clickable {
            let adopted = self
                .until(|| !editor.info_text().starts_with("Loading") && editor.selected_tile_id().is_some())
                .await;
            self.check(
                "choosing an online cover loads it",
                adopted,
                &editor.info_text(),
            );
            let info = editor.info_text();
            self.check(
                "the info line names the source and the pixel size",
                info.contains(" × "),
                &info,
            );
        }
    }

    async fn frame_the_red_corner_and_save(&self, editor: &Rc<Editor>) {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(quadrants())
            .write_to(&mut bytes, image::ImageFormat::Png)
            .expect("encode quadrants");
        editor.load_payload(Payload::Bytes(bytes.into_inner()), "Dropped image");
        let loaded = self
            .until(|| editor.info_text().starts_with("Dropped image"))
            .await;
        self.check("a dropped image replaces the cover", loaded, &editor.info_text());
        let crop = editor.crop();
        crop.fit();
        crop.zoom_by(4.0, Some((0.0, 0.0)));
        let framing = crop.framing().expect("framing");
        self.check(
            "zooming into a corner of a small image warns that it will look soft",
            framing.is_soft() && editor.soft_is_visible(),
            &format!("{:.0} source pixels showing", framing.visible_side()),
        );
        self.check(
            "a replaced image can be saved",
            editor.has_replaced_image() && editor.save_is_enabled(),
            "",
        );
        let expected_side = framing.output_pixels(crate::artwork_image::OUTPUT_LIMIT);
        let stale = self.cached_color().await;
        editor.save();
        let finished = self.until(|| editor.is_finished()).await;
        self.check("Save writes the cover and closes the editor", finished, "");
        let saved = self
            .ui
            .core
            .db
            .fetch_album_artwork(&self.album.title, &self.album.artist);
        let decoded = saved.as_deref().and_then(|bytes| crate::artwork_image::decode(bytes, 4000));
        self.check(
            "the database returns the saved cover first, a square JPEG at the framing's size",
            saved.as_deref().is_some_and(|bytes| bytes.starts_with(&[0xFF, 0xD8]))
                && decoded.as_ref().is_some_and(|image| image.dimensions() == (expected_side, expected_side)),
            &format!("{:?}", decoded.as_ref().map(|image| image.dimensions())),
        );
        let centre = decoded.as_ref().map(|image| {
            let side = image.width() / 2;
            *image.get_pixel(side, side)
        });
        self.check(
            "the saved cover shows the corner that was framed",
            centre.is_some_and(|pixel| pixel[0] > 200 && pixel[1] < 60 && pixel[2] < 60),
            &format!("{centre:?}"),
        );
        let keys = self.album.artwork_keys();
        self.check(
            "the cover is stored under every key a surface may ask by",
            self.ui.core.db.has_custom_cover(&keys)
                && keys.iter().all(|key| {
                    self.ui.core.db.has_custom_cover(std::slice::from_ref(key))
                }),
            &format!("{} keys", keys.len()),
        );
        let fresh = self.cached_color().await;
        self.check(
            "the artwork cache serves the new cover, not the one it held before",
            fresh != stale && fresh.is_some_and(|(r, g, b)| r > 150 && g < 100 && b < 100),
            &format!("{stale:?} -> {fresh:?}"),
        );
        let announced = self.announcements.borrow().len();
        self.check(
            "every surface is told once per key",
            announced == keys.len(),
            &format!("{announced} of {} keys", keys.len()),
        );
    }

    async fn after_save(
        &self,
        original: Option<Vec<u8>>,
        original_color: Option<(u8, u8, u8)>,
    ) {
        let editor = artwork_editor::present(&self.ui, &self.album, None);
        let loaded = self.until(|| editor.is_loaded()).await;
        self.check(
            "reopening shows the saved cover and offers Use Original Artwork",
            loaded && editor.reset_is_visible(),
            &editor.info_text(),
        );
        let shown = editor
            .crop()
            .source()
            .map(|source| *source.chain().full().get_pixel(source.width / 2, source.height / 2));
        self.check(
            "the reopened editor holds the saved cover",
            shown.is_some_and(|pixel| pixel[0] > 200 && pixel[1] < 60 && pixel[2] < 60),
            &format!("{shown:?}"),
        );
        editor.crop().scrolled(-4.0, 20.0, 20.0);
        let asks = editor.would_ask_before_closing();
        editor.close_with_discard();
        let closed = self.until(|| editor.is_finished()).await;
        self.check("discarding closes without writing", asks && closed, "");

        let before_events = self.announcements.borrow().len();
        let editor = artwork_editor::present(&self.ui, &self.album, None);
        self.until(|| editor.is_loaded()).await;
        editor.reset_now();
        let finished = self.until(|| editor.is_finished()).await;
        self.check("Use Original Artwork closes the editor", finished, "");
        let restored = self
            .ui
            .core
            .db
            .fetch_album_artwork(&self.album.title, &self.album.artist);
        self.check(
            "the original cover comes back",
            restored == original && !self.ui.core.db.has_custom_cover(&self.album.artwork_keys()),
            &format!("{} bytes", restored.as_ref().map_or(0, Vec::len)),
        );
        let back = self.cached_color().await;
        self.check(
            "the artwork cache serves the original again",
            back == original_color,
            &format!("{back:?} vs {original_color:?}"),
        );
        let announced = self.announcements.borrow().len() - before_events;
        self.check(
            "resetting tells every surface too",
            announced == self.album.artwork_keys().len(),
            &format!("{announced} events"),
        );
    }

    async fn cached_color(&self) -> Option<(u8, u8, u8)> {
        let answer: Rc<RefCell<Option<Dominant>>> = Rc::new(RefCell::new(None));
        let sink = Rc::clone(&answer);
        self.ui
            .core
            .artwork
            .request(&self.album.title, &self.album.artist, 168, move |_, color| {
                *sink.borrow_mut() = Some(color);
            });
        self.until(|| answer.borrow().is_some()).await;
        let color = *answer.borrow();
        color.flatten()
    }

    fn finish(&self) {
        if self.failures.get() == 0 {
            crate::logger::info("artwork", "ARTWORK DRIVE PASSED");
        } else {
            crate::logger::error(
                "artwork",
                &format!("ARTWORK DRIVE FAILED: {} checks", self.failures.get()),
            );
        }
    }
}

/// A 1200 px square in four flat colours, red in the top-left corner, so a
/// saved crop proves which part of the picture was framed.
fn quadrants() -> RgbaImage {
    RgbaImage::from_fn(1200, 1200, |x, y| match (x < 600, y < 600) {
        (true, true) => Rgba([230, 20, 20, 255]),
        (false, true) => Rgba([20, 200, 40, 255]),
        (true, false) => Rgba([30, 60, 230, 255]),
        (false, false) => Rgba([240, 220, 30, 255]),
    })
}
