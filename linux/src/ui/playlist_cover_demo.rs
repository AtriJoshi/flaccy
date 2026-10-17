//! Demo and verification hooks for playlist covers, driven from the
//! environment like the other `FLACCY_DEMO_*` switches:
//!
//! - `FLACCY_DEMO_PLAYLIST_COVER=seed` creates playlists drawing from one,
//!   two, three, four and many albums plus an empty one, then shows the list.
//! - `FLACCY_DEMO_PLAYLIST_COVER=detail` seeds and opens the many-album page.
//! - `FLACCY_DEMO_PLAYLIST_COVER=editor` seeds and opens the cover editor;
//!   `FLACCY_DEMO_PLAYLIST_COVER_IMAGE=<path>` opens it with that picture
//!   already framed, as a drop would.
//! - `FLACCY_DEMO_PLAYLIST_COVER=drive` picks a cover from the playlist shelf,
//!   saves it, reopens, resets it, and logs `PLAYLIST COVER DRIVE PASS` or
//!   `FAIL` for each check and `PASSED` or `FAILED` at the end.

use crate::events::AppEvent;
use crate::ui::artwork_editor::{self, Payload};
use crate::ui::Ui;
use flaccy_shared::playlist_cover;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(50);
const PATIENCE: Duration = Duration::from_secs(12);

const ALBUM_COUNTS: [(&str, usize); 6] = [
    ("Road Trip", 7),
    ("One Record", 1),
    ("Two Records", 2),
    ("Three Records", 3),
    ("Four Records", 4),
    ("Empty", 0),
];

pub fn open(ui: &Rc<Ui>, mode: &str) {
    let ids = seed(ui);
    let Some(&(_, road_trip)) = ids.iter().find(|(name, _)| *name == "Road Trip") else {
        crate::logger::warn("artwork", "playlist demo: the library has no albums to draw from");
        return;
    };
    match mode {
        "detail" => crate::ui::playlists::push_playlist_detail(ui, road_trip),
        "editor" => {
            let preload = std::env::var_os("FLACCY_DEMO_PLAYLIST_COVER_IMAGE")
                .map(std::path::PathBuf::from)
                .map(Payload::Path);
            artwork_editor::present_playlist(ui, road_trip, preload);
        }
        "drive" => {
            let ui = Rc::clone(ui);
            gtk::glib::spawn_future_local(async move {
                Drive::new(ui, road_trip).run().await;
            });
        }
        _ => {}
    }
}

/// Creates the demo playlists that do not exist yet, each taking the first
/// track of as many albums as its name promises, and returns every id.
fn seed(ui: &Rc<Ui>) -> Vec<(&'static str, i64)> {
    let library = ui.core.library.borrow().clone();
    let existing = ui.core.db.fetch_playlists();
    let mut ids = Vec::new();
    for (name, albums) in ALBUM_COUNTS {
        if let Some(found) = existing.iter().find(|playlist| playlist.name == name) {
            ids.push((name, found.id));
            continue;
        }
        let Ok(id) = ui.core.db.create_playlist(name) else {
            continue;
        };
        let paths: Vec<String> = library
            .albums
            .iter()
            .filter_map(|album| album.tracks.first())
            .take(albums)
            .map(|track| track.rel_path.clone())
            .collect();
        let _ = ui.core.db.add_tracks_to_playlist(id, &paths);
        ids.push((name, id));
    }
    ui.core.hub.emit(&AppEvent::LibraryReloaded);
    ids
}

struct Drive {
    ui: Rc<Ui>,
    playlist_id: i64,
    failures: Cell<usize>,
    announcements: Rc<RefCell<Vec<(String, String)>>>,
}

impl Drive {
    fn new(ui: Rc<Ui>, playlist_id: i64) -> Self {
        Self {
            ui,
            playlist_id,
            failures: Cell::new(0),
            announcements: Rc::new(RefCell::new(Vec::new())),
        }
    }

    fn check(&self, name: &str, passed: bool, detail: &str) {
        if passed {
            crate::logger::info("artwork", &format!("PLAYLIST COVER DRIVE PASS: {name} ({detail})"));
        } else {
            self.failures.set(self.failures.get() + 1);
            crate::logger::error("artwork", &format!("PLAYLIST COVER DRIVE FAIL: {name} ({detail})"));
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

    fn key(&self) -> (String, &'static str) {
        (playlist_cover::title_for(self.playlist_id), playlist_cover::ARTIST)
    }

    async fn run(self) {
        gtk::glib::timeout_future(Duration::from_millis(600)).await;
        let _ = self.ui.core.db.clear_playlist_cover(self.playlist_id);
        let seen = Rc::clone(&self.announcements);
        self.ui.core.hub.subscribe(move |event| {
            if let AppEvent::AlbumArtworkChanged { title, artist } = event {
                seen.borrow_mut().push((title.clone(), artist.clone()));
            }
            true
        });

        let editor = artwork_editor::present_playlist(&self.ui, self.playlist_id, None);
        self.check(
            "a playlist with no chosen cover opens empty with the search waiting for the person",
            editor.search_text().is_empty() && !editor.reset_is_visible(),
            &format!("search {:?}", editor.search_text()),
        );
        let hinted = self
            .until(|| {
                editor
                    .online_status_text()
                    .is_some_and(|text| text.contains("Search Apple Music"))
            })
            .await;
        self.check("the online shelf says what to do instead of searching a playlist name", hinted, "");
        let shelf = self.until(|| editor.files_tile_count() >= 4).await;
        self.check(
            "the shelf offers the albums the playlist draws from",
            shelf,
            &format!("{} tiles", editor.files_tile_count()),
        );
        let clicked = editor.click_first_files_tile();
        let adopted = clicked
            && self
                .until(|| !editor.info_text().starts_with("Loading") && editor.selected_tile_id().is_some())
                .await;
        self.check("choosing a shelf album loads it", adopted, &editor.info_text());
        self.check(
            "a chosen picture can be saved",
            editor.has_replaced_image() && editor.save_is_enabled(),
            "",
        );
        editor.save();
        let finished = self.until(|| editor.is_finished()).await;
        self.check("Save closes the editor", finished, "");

        let (title, artist) = self.key();
        let saved = self.ui.core.db.fetch_album_artwork(&title, artist);
        let decoded = saved.as_deref().and_then(|bytes| crate::artwork_image::decode(bytes, 4000));
        self.check(
            "the reserved key serves the saved square JPEG",
            saved.as_deref().is_some_and(|bytes| bytes.starts_with(&[0xFF, 0xD8]))
                && decoded.as_ref().is_some_and(|image| image.width() == image.height()),
            &format!("{:?}", decoded.as_ref().map(|image| image.dimensions())),
        );
        let announced = self.announcements.borrow().clone();
        self.check(
            "the surfaces are told once, under the playlist key",
            announced == vec![(title.clone(), artist.to_string())],
            &format!("{announced:?}"),
        );
        let served = self.served_by_cache().await;
        self.check("the artwork cache serves the new cover", served, "");

        let editor = artwork_editor::present_playlist(&self.ui, self.playlist_id, None);
        let loaded = self.until(|| editor.is_loaded()).await;
        self.check(
            "reopening shows the saved cover and offers Use Automatic Cover",
            loaded && editor.reset_is_visible(),
            &editor.info_text(),
        );
        editor.reset_now();
        let finished = self.until(|| editor.is_finished()).await;
        self.check("Use Automatic Cover closes the editor", finished, "");
        self.check(
            "the cover is gone and the mosaic is back",
            self.ui.core.db.playlist_cover(self.playlist_id).is_none()
                && !self.ui.core.db.has_playlist_cover(self.playlist_id),
            "",
        );
        let served = self.served_by_cache().await;
        self.check("the artwork cache no longer serves a cover", !served, "");
        self.check(
            "resetting tells the surfaces too",
            self.announcements.borrow().len() == 2,
            &format!("{} events", self.announcements.borrow().len()),
        );
        self.finish();
    }

    async fn served_by_cache(&self) -> bool {
        let answer: Rc<RefCell<Option<bool>>> = Rc::new(RefCell::new(None));
        let sink = Rc::clone(&answer);
        let (title, artist) = self.key();
        self.ui
            .core
            .artwork
            .request(&title, artist, 148, move |texture, _| {
                *sink.borrow_mut() = Some(texture.is_some());
            });
        self.until(|| answer.borrow().is_some()).await;
        let served = *answer.borrow();
        served.unwrap_or(false)
    }

    fn finish(&self) {
        if self.failures.get() == 0 {
            crate::logger::info("artwork", "PLAYLIST COVER DRIVE PASSED");
        } else {
            crate::logger::error(
                "artwork",
                &format!("PLAYLIST COVER DRIVE FAILED: {} checks", self.failures.get()),
            );
        }
    }
}
