use crate::events::AppEvent;
use crate::ui::mosaic::{AlbumRef, Mosaic, OnColor};
use crate::ui::Ui;
use adw::prelude::*;
use flaccy_shared::playlist_cover;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// A playlist's cover at any size: the picture the person chose when there is
/// one, otherwise a mosaic of up to four of its albums laid out like the Apple
/// clients (one full, two halves, one large beside two stacked, a 2×2 grid),
/// otherwise the palette placeholder. It repaints itself when the cover is
/// chosen or reset. A side of 0 makes the cover follow the width it is given,
/// and `bind` points a reused cover at another playlist.
#[derive(Clone)]
pub struct PlaylistArt {
    host: gtk::Overlay,
    state: Rc<State>,
}

const STATE_KEY: &str = "flaccy-playlist-art";

struct State {
    ui: Rc<Ui>,
    host: gtk::glib::WeakRef<gtk::Overlay>,
    playlist_id: Cell<i64>,
    name: RefCell<String>,
    albums: RefCell<Vec<AlbumRef>>,
    on_color: RefCell<Option<OnColor>>,
    token: Cell<u64>,
}

impl PlaylistArt {
    pub fn new(
        ui: &Rc<Ui>,
        playlist_id: i64,
        name: &str,
        side: i32,
        css_class: &str,
        albums: Vec<AlbumRef>,
    ) -> Self {
        let mosaic = Mosaic::new();
        mosaic.set_fixed_size(side);
        let host = mosaic.widget().clone();
        host.add_css_class(css_class);
        if side > 0 {
            host.set_halign(gtk::Align::Start);
            host.set_valign(gtk::Align::Start);
        }
        let state = Rc::new(State {
            ui: Rc::clone(ui),
            host: host.downgrade(),
            playlist_id: Cell::new(playlist_id),
            name: RefCell::new(name.to_string()),
            albums: RefCell::new(albums),
            on_color: RefCell::new(None),
            token: Cell::new(0),
        });
        unsafe {
            host.set_data(STATE_KEY, Rc::clone(&state));
        }
        state.show_mosaic();
        state.refresh();
        let weak = Rc::downgrade(&state);
        ui.core.hub.subscribe_widget(&host, move |_, event| {
            let AppEvent::AlbumArtworkChanged { title, artist } = event else {
                return;
            };
            let Some(state) = weak.upgrade() else { return };
            if playlist_cover::playlist_id(title, artist) == Some(state.playlist_id.get()) {
                state.refresh();
            }
        });
        Self { host, state }
    }

    /// Recovers the cover from the widget `widget()` returned, for virtualized
    /// factories that only keep the widget tree across row recycling.
    pub fn from_widget(widget: &gtk::Widget) -> Option<Self> {
        let host = widget.clone().downcast::<gtk::Overlay>().ok()?;
        let state = unsafe { host.data::<Rc<State>>(STATE_KEY)?.as_ref().clone() };
        Some(Self { host, state })
    }

    pub fn widget(&self) -> gtk::Overlay {
        self.host.clone()
    }

    /// Resizes a pinned cover.
    pub fn set_side(&self, side: i32) {
        if let Some(mosaic) = self.state.mosaic() {
            mosaic.set_fixed_size(side);
        }
        self.host.set_halign(gtk::Align::Start);
        self.host.set_valign(gtk::Align::Start);
    }

    /// Reports the dominant color of whatever the cover shows, for a page that
    /// tints itself after its cover.
    pub fn set_on_color(&self, on_color: OnColor) {
        *self.state.on_color.borrow_mut() = Some(on_color);
        self.state.refresh();
    }

    /// Points the cover at another playlist and repaints it.
    pub fn bind(&self, playlist_id: i64, name: &str, albums: Vec<AlbumRef>) {
        self.state.playlist_id.set(playlist_id);
        *self.state.name.borrow_mut() = name.to_string();
        *self.state.albums.borrow_mut() = albums;
        self.state.show_mosaic();
        self.state.refresh();
    }
}

impl State {
    fn mosaic(&self) -> Option<Mosaic> {
        Mosaic::from_root(&self.host.upgrade()?)
    }

    fn refresh(self: &Rc<Self>) {
        let Some(mosaic) = self.mosaic() else { return };
        let token = self.token.get() + 1;
        self.token.set(token);
        let weak = Rc::downgrade(self);
        self.ui.core.artwork.request(
            &playlist_cover::title_for(self.playlist_id.get()),
            playlist_cover::ARTIST,
            mosaic.single_px(),
            move |texture, color| {
                let Some(state) = weak.upgrade() else { return };
                if token != state.token.get() {
                    return;
                }
                match texture {
                    Some(texture) => {
                        if let Some(mosaic) = state.mosaic() {
                            mosaic.set_picture(texture);
                        }
                        let on_color = state.on_color.borrow().clone();
                        if let (Some(on_color), Some(color)) = (on_color, color) {
                            on_color(color);
                        }
                    }
                    None => state.show_mosaic(),
                }
            },
        );
    }

    fn show_mosaic(&self) {
        let Some(mosaic) = self.mosaic() else { return };
        let name = self.name.borrow().clone();
        let albums = self.albums.borrow().clone();
        let on_color = self.on_color.borrow().clone();
        mosaic.set_covers(&self.ui, &name, &albums, on_color);
    }
}

/// Asks before taking a chosen cover away, then lets the playlist show its
/// automatic mosaic again.
pub fn confirm_reset(ui: &Rc<Ui>, playlist_id: i64) {
    let dialog = adw::AlertDialog::builder()
        .heading("Use the automatic cover?")
        .body("Your chosen cover will be removed and the playlist shows a mosaic of its albums again.")
        .close_response("cancel")
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("reset", "Use Automatic");
    dialog.set_response_appearance("reset", adw::ResponseAppearance::Destructive);
    let window = ui.window.clone();
    let ui = Rc::clone(ui);
    dialog.connect_response(None, move |_, response| {
        if response != "reset" {
            return;
        }
        match ui.core.db.clear_playlist_cover(playlist_id) {
            Ok(()) => {
                ui.core.album_artwork_changed(&[(
                    playlist_cover::title_for(playlist_id),
                    playlist_cover::ARTIST.to_string(),
                )]);
                ui.core.toast("Automatic cover restored");
            }
            Err(err) => {
                crate::logger::error(
                    "artwork",
                    &format!("resetting playlist cover failed: {err}"),
                );
                ui.core.toast("Couldn't restore the automatic cover");
            }
        }
    });
    dialog.present(Some(&window));
}
