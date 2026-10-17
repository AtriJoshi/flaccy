use crate::artwork_image::MipChain;
use flaccy_shared::artwork_framing::ArtworkFraming;
use gtk::prelude::*;
use gtk::{cairo, gdk, glib};
use image::RgbaImage;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const PREFERRED_SIDE: i32 = 400;
pub const MINIMUM_SIDE: i32 = 220;
pub const KEY_STEP: f64 = 10.0;
pub const KEY_STEP_LARGE: f64 = 40.0;
pub const KEY_ZOOM_FACTOR: f64 = 1.1;
const WHEEL_ZOOM_RATE: f64 = 0.09;
const GUIDE_HOLD: Duration = Duration::from_millis(350);
const GUIDE_FRAME: Duration = Duration::from_millis(16);
const FRAME_RADIUS: f64 = 10.0;

/// A decoded source image and everything needed to paint it at any framing:
/// the reduced levels, and each level as a Cairo surface made on first use.
pub struct ArtworkSource {
    chain: Arc<MipChain>,
    surfaces: RefCell<Vec<Option<cairo::ImageSurface>>>,
    pub width: u32,
    pub height: u32,
}

impl ArtworkSource {
    pub fn new(chain: MipChain) -> Rc<Self> {
        let (width, height) = chain.full().dimensions();
        Rc::new(Self {
            surfaces: RefCell::new(vec![None; chain.level_count()]),
            chain: Arc::new(chain),
            width,
            height,
        })
    }

    /// The decoded levels, shareable with a worker that renders the cover.
    pub fn chain(&self) -> Arc<MipChain> {
        Arc::clone(&self.chain)
    }

    fn surface(&self, level: usize) -> Option<cairo::ImageSurface> {
        let mut surfaces = self.surfaces.borrow_mut();
        let slot = surfaces.get_mut(level)?;
        if slot.is_none() {
            *slot = surface_from(self.chain.level(level));
        }
        slot.clone()
    }

    /// Paints what `framing` shows into a `side` × `side` square at the
    /// origin. The crop frame and every preview go through this one routine,
    /// so a preview can never disagree with the frame it previews.
    pub fn paint(&self, cr: &cairo::Context, framing: &ArtworkFraming, device_scale: f64) {
        let (level, factor) = self.chain.level_for(framing.zoom() * device_scale);
        let Some(surface) = self.surface(level) else {
            return;
        };
        let _ = cr.save();
        let scale = framing.zoom() * factor;
        cr.scale(scale, scale);
        cr.translate(-framing.visible_x() / factor, -framing.visible_y() / factor);
        let _ = cr.set_source_surface(&surface, 0.0, 0.0);
        cr.source().set_extend(cairo::Extend::Pad);
        cr.source().set_filter(cairo::Filter::Good);
        let _ = cr.paint();
        let _ = cr.restore();
    }

    /// The image blown up past the window as a soft wash: its smallest level,
    /// scaled to cover, so it costs nothing and blurs by itself.
    pub fn paint_backdrop(&self, cr: &cairo::Context, width: f64, height: f64) {
        let level = self.surfaces.borrow().len().saturating_sub(1);
        let Some(surface) = self.surface(level) else {
            return;
        };
        let (iw, ih) = (surface.width() as f64, surface.height() as f64);
        let scale = (width / iw).max(height / ih);
        let _ = cr.save();
        cr.translate((width - iw * scale) / 2.0, (height - ih * scale) / 2.0);
        cr.scale(scale, scale);
        let _ = cr.set_source_surface(&surface, 0.0, 0.0);
        cr.source().set_filter(cairo::Filter::Bilinear);
        let _ = cr.paint();
        let _ = cr.restore();
    }

}

/// Premultiplied native-endian ARGB, which is what Cairo paints.
fn surface_from(image: &RgbaImage) -> Option<cairo::ImageSurface> {
    let (width, height) = image.dimensions();
    let stride = cairo::Format::ARgb32.stride_for_width(width).ok()?;
    let mut data = vec![0u8; stride as usize * height as usize];
    for (row, line) in image.rows().enumerate() {
        let out = &mut data[row * stride as usize..];
        for (column, pixel) in line.enumerate() {
            let alpha = pixel[3] as u32;
            let premultiply = |channel: u8| ((channel as u32 * alpha + 127) / 255) as u8;
            let argb = (alpha << 24)
                | (premultiply(pixel[0]) as u32) << 16
                | (premultiply(pixel[1]) as u32) << 8
                | premultiply(pixel[2]) as u32;
            out[column * 4..column * 4 + 4].copy_from_slice(&argb.to_ne_bytes());
        }
    }
    cairo::ImageSurface::create_for_data(
        data,
        cairo::Format::ARgb32,
        width as i32,
        height as i32,
        stride,
    )
    .ok()
}

pub fn rounded_rectangle(cr: &cairo::Context, x: f64, y: f64, width: f64, height: f64, radius: f64) {
    let radius = radius.min(width / 2.0).min(height / 2.0);
    let quarter = std::f64::consts::FRAC_PI_2;
    cr.new_sub_path();
    cr.arc(x + width - radius, y + radius, radius, -quarter, 0.0);
    cr.arc(x + width - radius, y + height - radius, radius, 0.0, quarter);
    cr.arc(x + radius, y + height - radius, radius, quarter, 2.0 * quarter);
    cr.arc(x + radius, y + radius, radius, 2.0 * quarter, 3.0 * quarter);
    cr.close_path();
}

/// The crop frame: a square window onto the source image that is dragged,
/// wheel-zoomed, pinched, double-clicked and nudged from the keyboard. Every
/// gesture is a method here that does one thing to the shared `ArtworkFraming`,
/// so a drive script and a real pointer run the same code.
pub struct CropView {
    area: gtk::DrawingArea,
    source: RefCell<Option<Rc<ArtworkSource>>>,
    framing: Cell<Option<ArtworkFraming>>,
    drag_origin: Rc<Cell<Option<ArtworkFraming>>>,
    pinch_origin: Cell<Option<ArtworkFraming>>,
    pointer: Cell<(f64, f64)>,
    loading: Cell<bool>,
    guide_alpha: Rc<Cell<f64>>,
    guide_until: Rc<Cell<Option<Instant>>>,
    guide_ticker: Rc<Cell<bool>>,
    changed: RefCell<Vec<Rc<dyn Fn()>>>,
}

impl CropView {
    pub fn new() -> Rc<Self> {
        let area = gtk::DrawingArea::builder()
            .focusable(true)
            .can_focus(true)
            .hexpand(true)
            .vexpand(true)
            .content_width(MINIMUM_SIDE)
            .content_height(MINIMUM_SIDE)
            .build();
        area.add_css_class("artwork-crop");
        area.set_cursor_from_name(Some("grab"));
        area.update_property(&[
            gtk::accessible::Property::Label("Artwork preview"),
            gtk::accessible::Property::Description(
                "Drag to move the cover, scroll to zoom, arrow keys nudge, plus and minus zoom, 0 fits",
            ),
        ]);
        let view = Rc::new(Self {
            area,
            source: RefCell::new(None),
            framing: Cell::new(None),
            drag_origin: Rc::new(Cell::new(None)),
            pinch_origin: Cell::new(None),
            pointer: Cell::new((0.0, 0.0)),
            loading: Cell::new(false),
            guide_alpha: Rc::new(Cell::new(0.0)),
            guide_until: Rc::new(Cell::new(None)),
            guide_ticker: Rc::new(Cell::new(false)),
            changed: RefCell::new(Vec::new()),
        });
        view.install_drawing();
        view.install_gestures();
        view
    }

    pub fn area(&self) -> &gtk::DrawingArea {
        &self.area
    }

    pub fn source(&self) -> Option<Rc<ArtworkSource>> {
        self.source.borrow().clone()
    }

    pub fn framing(&self) -> Option<ArtworkFraming> {
        self.framing.get()
    }

    pub fn has_image(&self) -> bool {
        self.source.borrow().is_some()
    }

    pub fn connect_changed(&self, handler: impl Fn() + 'static) {
        self.changed.borrow_mut().push(Rc::new(handler));
    }

    fn notify(&self) {
        self.area.queue_draw();
        let handlers: Vec<Rc<dyn Fn()>> = self.changed.borrow().clone();
        for handler in handlers {
            handler();
        }
    }

    pub fn side(&self) -> f64 {
        let (width, height) = (self.area.width(), self.area.height());
        let side = width.min(height);
        if side > 0 {
            side as f64
        } else {
            PREFERRED_SIDE as f64
        }
    }

    /// Frames a new image filled and centred, the way a freshly loaded cover
    /// starts.
    pub fn set_source(&self, source: Option<Rc<ArtworkSource>>) {
        let framing = source
            .as_ref()
            .and_then(|source| ArtworkFraming::new(source.width as f64, source.height as f64, self.side()));
        *self.source.borrow_mut() = source;
        self.framing.set(framing);
        self.notify();
    }

    pub fn set_loading(&self, loading: bool) {
        self.loading.set(loading);
        self.area.queue_draw();
    }

    /// Whether the person has moved the picture away from filled and centred.
    pub fn is_pristine(&self) -> bool {
        let Some(framing) = self.framing.get() else {
            return true;
        };
        framing.is_at_fill()
            && (framing.center_x() - framing.image_width / 2.0).abs() < 0.5
            && (framing.center_y() - framing.image_height / 2.0).abs() < 0.5
    }

    fn update(&self, next: impl FnOnce(&ArtworkFraming) -> ArtworkFraming) {
        let Some(current) = self.framing.get() else {
            return;
        };
        let framing = next(&current);
        if framing == current {
            return;
        }
        self.framing.set(Some(framing));
        self.touch_guides();
        self.notify();
    }

    pub fn resize(&self, side: f64) {
        let Some(current) = self.framing.get() else {
            return;
        };
        let framing = current.resized(side);
        if framing != current {
            self.framing.set(Some(framing));
            self.notify();
        }
    }

    pub fn pan(&self, dx: f64, dy: f64) {
        self.update(|framing| framing.panned(dx, dy));
    }

    pub fn zoom_by(&self, factor: f64, anchor: Option<(f64, f64)>) {
        self.update(|framing| framing.zoomed_by(factor, anchor));
    }

    pub fn zoom_to_fraction(&self, fraction: f64) {
        self.update(|framing| framing.zoomed_to_fraction(fraction));
    }

    pub fn toggle_zoom(&self, x: f64, y: f64) {
        self.update(|framing| framing.toggled(x, y));
    }

    pub fn fit(&self) {
        let Some(source) = self.source() else {
            return;
        };
        let fresh = ArtworkFraming::new(source.width as f64, source.height as f64, self.side());
        if fresh != self.framing.get() {
            self.framing.set(fresh);
            self.touch_guides();
            self.notify();
        }
    }

    pub fn drag_begin(&self) {
        self.drag_origin.set(self.framing.get());
        self.area.set_cursor_from_name(Some("grabbing"));
        self.touch_guides();
    }

    pub fn drag_update(&self, dx: f64, dy: f64) {
        let Some(origin) = self.drag_origin.get() else {
            return;
        };
        let next = origin.panned(dx, dy);
        if Some(next) != self.framing.get() {
            self.framing.set(Some(next));
            self.touch_guides();
            self.notify();
        }
    }

    pub fn drag_end(&self) {
        self.drag_origin.set(None);
        self.area.set_cursor_from_name(Some("grab"));
        self.touch_guides();
    }

    pub fn scrolled(&self, dy: f64, x: f64, y: f64) {
        self.zoom_by((-dy * WHEEL_ZOOM_RATE).exp(), Some((x, y)));
    }

    pub fn pinch_begin(&self) {
        self.pinch_origin.set(self.framing.get());
    }

    pub fn pinch_scale(&self, scale: f64, anchor: (f64, f64)) {
        let Some(origin) = self.pinch_origin.get() else {
            return;
        };
        let next = origin.zoomed_by(scale, Some(anchor));
        if Some(next) != self.framing.get() {
            self.framing.set(Some(next));
            self.touch_guides();
            self.notify();
        }
    }

    /// Arrow keys nudge the picture the way they point (10 points, 40 with
    /// Shift), plus and minus zoom by a tenth, 0 fits. Returns whether the key
    /// was one of these.
    pub fn key_pressed(&self, key: gdk::Key, state: gdk::ModifierType) -> bool {
        if state.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK) {
            return false;
        }
        let step = if state.contains(gdk::ModifierType::SHIFT_MASK) {
            KEY_STEP_LARGE
        } else {
            KEY_STEP
        };
        let centre = (self.side() / 2.0, self.side() / 2.0);
        match key {
            gdk::Key::Left | gdk::Key::KP_Left => self.pan(-step, 0.0),
            gdk::Key::Right | gdk::Key::KP_Right => self.pan(step, 0.0),
            gdk::Key::Up | gdk::Key::KP_Up => self.pan(0.0, -step),
            gdk::Key::Down | gdk::Key::KP_Down => self.pan(0.0, step),
            gdk::Key::plus | gdk::Key::equal | gdk::Key::KP_Add => {
                self.zoom_by(KEY_ZOOM_FACTOR, Some(centre))
            }
            gdk::Key::minus | gdk::Key::underscore | gdk::Key::KP_Subtract => {
                self.zoom_by(1.0 / KEY_ZOOM_FACTOR, Some(centre))
            }
            gdk::Key::_0 | gdk::Key::KP_0 => self.fit(),
            _ => return false,
        }
        true
    }

    /// Thirds guides appear while the frame is being worked and fade out
    /// shortly after.
    fn touch_guides(&self) {
        self.guide_until.set(Some(Instant::now() + GUIDE_HOLD));
        self.guide_alpha.set(self.guide_alpha.get().max(0.35));
        if self.guide_ticker.replace(true) {
            return;
        }
        let area = self.area.downgrade();
        let alpha = Rc::clone(&self.guide_alpha);
        let until = Rc::clone(&self.guide_until);
        let ticker = Rc::clone(&self.guide_ticker);
        let dragging = Rc::clone(&self.drag_origin);
        glib::timeout_add_local(GUIDE_FRAME, move || {
            let Some(area) = area.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let held = dragging.get().is_some() || until.get().is_some_and(|at| Instant::now() < at);
            let target = if held { 1.0 } else { 0.0 };
            let next = alpha.get() + (target - alpha.get()) * 0.32;
            alpha.set(if (target - next).abs() < 0.01 { target } else { next });
            area.queue_draw();
            if !held && alpha.get() == 0.0 {
                ticker.set(false);
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
    }

    fn install_drawing(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.area.set_draw_func(move |area, cr, width, height| {
            let Some(view) = weak.upgrade() else { return };
            view.draw(area, cr, width as f64, height as f64);
        });
        let resize = Rc::downgrade(self);
        self.area.connect_resize(move |_, width, height| {
            if let Some(view) = resize.upgrade() {
                view.resize(width.min(height) as f64);
            }
        });
    }

    fn draw(&self, area: &gtk::DrawingArea, cr: &cairo::Context, width: f64, height: f64) {
        let side = width.min(height);
        let (x, y) = ((width - side) / 2.0, (height - side) / 2.0);
        let _ = cr.save();
        cr.translate(x, y);
        rounded_rectangle(cr, 0.0, 0.0, side, side, FRAME_RADIUS);
        cr.clip_preserve();
        cr.set_source_rgba(0.5, 0.5, 0.5, 0.14);
        let _ = cr.fill();
        if let (Some(source), Some(framing)) = (self.source(), self.framing.get()) {
            source.paint(cr, &framing, area.scale_factor() as f64);
        }
        if self.loading.get() {
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.45);
            let _ = cr.paint();
        }
        self.draw_guides(cr, side);
        let _ = cr.restore();
        let _ = cr.save();
        cr.translate(x, y);
        rounded_rectangle(cr, 0.5, 0.5, side - 1.0, side - 1.0, FRAME_RADIUS);
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.12);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
        let _ = cr.restore();
    }

    fn draw_guides(&self, cr: &cairo::Context, side: f64) {
        let alpha = self.guide_alpha.get();
        if alpha <= 0.0 || !self.has_image() {
            return;
        }
        cr.set_line_width(1.0);
        for third in [1.0 / 3.0, 2.0 / 3.0] {
            let offset = (side * third).round() + 0.5;
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.25 * alpha);
            cr.move_to(offset + 1.0, 0.0);
            cr.line_to(offset + 1.0, side);
            cr.move_to(0.0, offset + 1.0);
            cr.line_to(side, offset + 1.0);
            let _ = cr.stroke();
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.55 * alpha);
            cr.move_to(offset, 0.0);
            cr.line_to(offset, side);
            cr.move_to(0.0, offset);
            cr.line_to(side, offset);
            let _ = cr.stroke();
        }
    }

    fn install_gestures(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);

        let drag = gtk::GestureDrag::builder().button(gdk::BUTTON_PRIMARY).build();
        {
            let view = weak.clone();
            drag.connect_drag_begin(move |_, _, _| {
                if let Some(view) = view.upgrade() {
                    view.area.grab_focus();
                    view.drag_begin();
                }
            });
        }
        {
            let view = weak.clone();
            drag.connect_drag_update(move |_, dx, dy| {
                if let Some(view) = view.upgrade() {
                    view.drag_update(dx, dy);
                }
            });
        }
        {
            let view = weak.clone();
            drag.connect_drag_end(move |_, _, _| {
                if let Some(view) = view.upgrade() {
                    view.drag_end();
                }
            });
        }
        self.area.add_controller(drag);

        let click = gtk::GestureClick::builder().button(gdk::BUTTON_PRIMARY).build();
        {
            let view = weak.clone();
            click.connect_pressed(move |_, presses, x, y| {
                if presses == 2 {
                    if let Some(view) = view.upgrade() {
                        view.toggle_zoom(x, y);
                    }
                }
            });
        }
        self.area.add_controller(click);

        let motion = gtk::EventControllerMotion::new();
        {
            let view = weak.clone();
            motion.connect_motion(move |_, x, y| {
                if let Some(view) = view.upgrade() {
                    view.pointer.set((x, y));
                }
            });
        }
        self.area.add_controller(motion);

        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        {
            let view = weak.clone();
            scroll.connect_scroll(move |_, _, dy| {
                let Some(view) = view.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                let (x, y) = view.pointer.get();
                view.scrolled(dy, x, y);
                glib::Propagation::Stop
            });
        }
        self.area.add_controller(scroll);

        let zoom = gtk::GestureZoom::new();
        {
            let view = weak.clone();
            zoom.connect_begin(move |_, _| {
                if let Some(view) = view.upgrade() {
                    view.pinch_begin();
                }
            });
        }
        {
            let view = weak.clone();
            zoom.connect_scale_changed(move |gesture, scale| {
                let Some(view) = view.upgrade() else { return };
                let anchor = gesture
                    .bounding_box_center()
                    .unwrap_or((view.side() / 2.0, view.side() / 2.0));
                view.pinch_scale(scale, anchor);
            });
        }
        self.area.add_controller(zoom);

        let keys = gtk::EventControllerKey::new();
        {
            let view = weak.clone();
            keys.connect_key_pressed(move |_, key, _, state| {
                let handled = view
                    .upgrade()
                    .is_some_and(|view| view.key_pressed(key, state));
                if handled {
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            });
        }
        self.area.add_controller(keys);
    }
}

/// A small live rendering of the crop frame's framing, so the person sees the
/// cover as it will look as an album tile, a list row or the mini player.
pub fn preview_area(crop: &Rc<CropView>, side: i32, radius: f64) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder()
        .content_width(side)
        .content_height(side)
        .halign(gtk::Align::Start)
        .valign(gtk::Align::End)
        .build();
    let weak = Rc::downgrade(crop);
    area.set_draw_func(move |area, cr, width, height| {
        let _ = width.min(height);
        let Some(crop) = weak.upgrade() else { return };
        let side = width.min(height) as f64;
        rounded_rectangle(cr, 0.0, 0.0, side, side, radius);
        cr.clip_preserve();
        cr.set_source_rgba(0.5, 0.5, 0.5, 0.14);
        let _ = cr.fill();
        if let (Some(source), Some(framing)) = (crop.source(), crop.framing()) {
            source.paint(cr, &framing.resized(side), area.scale_factor() as f64);
        }
    });
    let weak = Rc::downgrade(crop);
    let preview = area.downgrade();
    crop.connect_changed(move || {
        if weak.upgrade().is_some() {
            if let Some(preview) = preview.upgrade() {
                preview.queue_draw();
            }
        }
    });
    area
}

/// A shelf thumbnail: a fixed square that paints its picture through the same
/// routine as the crop frame, filled and centred.
#[derive(Clone)]
pub struct ThumbArea {
    area: gtk::DrawingArea,
    source: Rc<RefCell<Option<Rc<ArtworkSource>>>>,
}

impl ThumbArea {
    pub fn new(side: i32, radius: f64) -> Self {
        let area = gtk::DrawingArea::builder()
            .content_width(side)
            .content_height(side)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Start)
            .build();
        area.add_css_class("artwork-tile-picture");
        let source: Rc<RefCell<Option<Rc<ArtworkSource>>>> = Rc::new(RefCell::new(None));
        let shown = Rc::clone(&source);
        area.set_draw_func(move |area, cr, width, height| {
            let side = width.min(height) as f64;
            rounded_rectangle(cr, 0.0, 0.0, side, side, radius);
            cr.clip_preserve();
            cr.set_source_rgba(0.5, 0.5, 0.5, 0.18);
            let _ = cr.fill();
            let Some(source) = shown.borrow().clone() else {
                return;
            };
            if let Some(framing) = ArtworkFraming::new(source.width as f64, source.height as f64, side) {
                source.paint(cr, &framing, area.scale_factor() as f64);
            }
        });
        Self { area, source }
    }

    pub fn area(&self) -> &gtk::DrawingArea {
        &self.area
    }

    pub fn set_image(&self, image: RgbaImage) {
        *self.source.borrow_mut() = Some(ArtworkSource::new(MipChain::new(image)));
        self.area.queue_draw();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn a_surface_is_premultiplied_native_argb() {
        let mut image = RgbaImage::new(2, 1);
        image.put_pixel(0, 0, Rgba([200, 100, 50, 255]));
        image.put_pixel(1, 0, Rgba([200, 100, 50, 128]));
        let mut surface = surface_from(&image).expect("surface");
        let stride = surface.stride() as usize;
        let data = surface.data().expect("data");
        let pixel = |column: usize| {
            u32::from_ne_bytes(data[column * 4..column * 4 + 4].try_into().expect("four bytes"))
        };
        assert_eq!(pixel(0), 0xFFC8_6432u32);
        assert_eq!(pixel(1) >> 24, 128);
        assert_eq!((pixel(1) >> 16) & 0xFF, 100, "200 * 128 / 255");
        assert!(stride >= 8);
    }
}
