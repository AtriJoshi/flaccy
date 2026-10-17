use crate::events::AppEvent;
use crate::ui::{artwork, Ui};
use adw::prelude::*;
use flaccy_shared::playlist_cover;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// A playlist's cover at any size: the picture the person chose when there is
/// one, otherwise a mosaic of up to four of its albums laid out like the Apple
/// clients (one full, two halves, one large beside two stacked, a 2×2 grid),
/// otherwise the palette placeholder. It repaints itself when the cover is
/// chosen or reset.
pub struct PlaylistArt {
    host: gtk::Overlay,
    state: Rc<State>,
}

const STATE_KEY: &str = "flaccy-playlist-art";

struct State {
    ui: Rc<Ui>,
    playlist_id: i64,
    name: String,
    side: i32,
    host: gtk::glib::WeakRef<gtk::Overlay>,
    content: RefCell<Option<gtk::Widget>>,
    albums: RefCell<Vec<(String, String)>>,
    token: Cell<u64>,
}

impl PlaylistArt {
    pub fn new(
        ui: &Rc<Ui>,
        playlist_id: i64,
        name: &str,
        side: i32,
        css_class: &str,
        albums: Vec<(String, String)>,
    ) -> Self {
        let spacer = gtk::Box::builder()
            .width_request(side)
            .height_request(side)
            .build();
        let host = gtk::Overlay::builder()
            .child(&spacer)
            .hexpand(false)
            .vexpand(false)
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Start)
            .overflow(gtk::Overflow::Hidden)
            .build();
        host.add_css_class(css_class);
        let state = Rc::new(State {
            ui: Rc::clone(ui),
            playlist_id,
            name: name.to_string(),
            side,
            host: host.downgrade(),
            content: RefCell::new(None),
            albums: RefCell::new(albums),
            token: Cell::new(0),
        });
        unsafe {
            host.set_data(STATE_KEY, Rc::clone(&state));
        }
        state.refresh();
        let weak = Rc::downgrade(&state);
        ui.core.hub.subscribe_widget(&host, move |_, event| {
            let AppEvent::AlbumArtworkChanged { title, artist } = event else {
                return;
            };
            let Some(state) = weak.upgrade() else { return };
            if playlist_cover::playlist_id(title, artist) == Some(state.playlist_id) {
                state.refresh();
            }
        });
        Self { host, state }
    }

    pub fn widget(&self) -> gtk::Overlay {
        self.host.clone()
    }

    /// Replaces the albums the automatic mosaic is drawn from, repainting only
    /// when they changed.
    pub fn set_albums(&self, albums: Vec<(String, String)>) {
        if *self.state.albums.borrow() == albums {
            return;
        }
        *self.state.albums.borrow_mut() = albums;
        self.state.refresh();
    }
}

impl State {
    fn refresh(self: &Rc<Self>) {
        let token = self.token.get() + 1;
        self.token.set(token);
        let weak = Rc::downgrade(self);
        self.ui.core.artwork.request(
            &playlist_cover::title_for(self.playlist_id),
            playlist_cover::ARTIST,
            self.side as u32,
            move |texture, _| {
                let Some(state) = weak.upgrade() else { return };
                if token != state.token.get() {
                    return;
                }
                match texture {
                    Some(texture) => state.show_picture(texture),
                    None => state.show_mosaic(),
                }
            },
        );
    }

    /// Replaces what the cover shows. The content is an overlay over a spacer
    /// of exactly the cover's side, so however large a texture is, it is
    /// allocated that square and nothing more.
    fn show(&self, content: &impl IsA<gtk::Widget>) {
        let Some(host) = self.host.upgrade() else { return };
        if let Some(previous) = self.content.borrow_mut().take() {
            host.remove_overlay(&previous);
        }
        host.add_overlay(content);
        host.set_clip_overlay(content, true);
        *self.content.borrow_mut() = Some(content.clone().upcast());
    }

    fn show_picture(&self, texture: &gtk::gdk::MemoryTexture) {
        let picture = fill_picture();
        picture.set_paintable(Some(texture));
        self.show(&picture);
    }

    fn show_mosaic(&self) {
        let albums = self.albums.borrow().clone();
        if albums.is_empty() {
            let picture = fill_picture();
            picture.set_paintable(Some(&self.ui.core.artwork.placeholder(&self.name)));
            self.show(&picture);
            return;
        }
        let request = if albums.len() == 1 { self.side } else { self.side / 2 } as u32;
        let tiles: Vec<gtk::Picture> = albums
            .iter()
            .map(|(title, artist)| {
                let picture = fill_picture();
                picture.set_paintable(Some(
                    &self.ui.core.artwork.placeholder(&format!("{title}|{artist}")),
                ));
                artwork::follow_picture(&self.ui.core, &picture, title, artist, request);
                picture
            })
            .collect();
        self.show(&layout(&tiles));
    }
}

/// A picture that fills whatever it is given, cropping the overflow.
fn fill_picture() -> gtk::Picture {
    let picture = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .can_shrink(true)
        .hexpand(true)
        .vexpand(true)
        .build();
    picture.set_overflow(gtk::Overflow::Hidden);
    picture
}

fn split(orientation: gtk::Orientation, children: &[&gtk::Widget]) -> gtk::Box {
    let container = gtk::Box::builder()
        .orientation(orientation)
        .homogeneous(true)
        .hexpand(true)
        .vexpand(true)
        .build();
    for child in children {
        container.append(*child);
    }
    container
}

/// One tile fills the square, two split it in halves, three give the first a
/// half and stack the others beside it, four make a grid.
fn layout(tiles: &[gtk::Picture]) -> gtk::Widget {
    use gtk::Orientation::{Horizontal, Vertical};
    let widget = |index: usize| tiles[index].clone().upcast::<gtk::Widget>();
    match tiles.len() {
        1 => widget(0),
        2 => split(Horizontal, &[&widget(0), &widget(1)]).upcast(),
        3 => {
            let column = split(Vertical, &[&widget(1), &widget(2)]);
            split(Horizontal, &[&widget(0), column.upcast_ref()]).upcast()
        }
        _ => {
            let top = split(Horizontal, &[&widget(0), &widget(1)]);
            let bottom = split(Horizontal, &[&widget(2), &widget(3)]);
            split(Vertical, &[top.upcast_ref(), bottom.upcast_ref()]).upcast()
        }
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
                crate::logger::error("artwork", &format!("resetting playlist cover failed: {err}"));
                ui.core.toast("Couldn't restore the automatic cover");
            }
        }
    });
    dialog.present(Some(&window));
}
