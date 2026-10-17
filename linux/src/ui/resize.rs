use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Runs `on_change(width, previous)` once a layout pass has left `widget` at a
/// new width, and never while the layout is at rest. `previous` is zero the
/// first time.
///
/// This replaces the widget tick callback these checks used to be: a tick
/// callback, however little it does, asks the frame clock for a cycle on every
/// vertical blank for as long as the widget lives — 165 wakeups a second on a
/// fast display, with nothing on screen moving. The frame clock's layout phase
/// only runs when something asked for a layout, which is exactly when a width
/// can have changed. The phase also repeats until it settles, so whatever the
/// callback shows or hides is laid out in the same frame, with no flash of the
/// old arrangement.
pub fn watch_width(widget: &impl IsA<gtk::Widget>, on_change: impl Fn(i32, i32) + 'static) {
    let watch = Rc::new(Watch {
        last: Cell::new(0),
        on_change: Box::new(on_change),
        attached: RefCell::new(None),
    });
    let widget = widget.as_ref();
    {
        let watch = Rc::clone(&watch);
        widget.connect_realize(move |widget| watch.attach(widget));
    }
    {
        let watch = Rc::clone(&watch);
        widget.connect_unrealize(move |_| watch.detach());
    }
    if widget.is_realized() {
        watch.attach(widget);
    }
}

struct Watch {
    last: Cell<i32>,
    on_change: Box<dyn Fn(i32, i32)>,
    attached: RefCell<Option<(gtk::gdk::FrameClock, glib::SignalHandlerId)>>,
}

impl Watch {
    /// Listens to the toplevel's frame clock. The handler is connected after
    /// the toolkit's own layout handler, so it reads the widths that layout
    /// just assigned.
    fn attach(self: &Rc<Self>, widget: &gtk::Widget) {
        self.detach();
        let Some(clock) = widget.frame_clock() else {
            return;
        };
        let (watch, weak) = (Rc::downgrade(self), widget.downgrade());
        let id = clock.connect_layout(move |_| {
            if let (Some(watch), Some(widget)) = (watch.upgrade(), weak.upgrade()) {
                watch.check(&widget);
            }
        });
        *self.attached.borrow_mut() = Some((clock, id));
        let (watch, weak) = (Rc::downgrade(self), widget.downgrade());
        glib::idle_add_local_once(move || {
            if let (Some(watch), Some(widget)) = (watch.upgrade(), weak.upgrade()) {
                watch.check(&widget);
            }
        });
    }

    fn detach(&self) {
        if let Some((clock, id)) = self.attached.borrow_mut().take() {
            clock.disconnect(id);
        }
    }

    fn check(&self, widget: &gtk::Widget) {
        let width = widget.width();
        if width <= 1 || width == self.last.get() {
            return;
        }
        let previous = self.last.replace(width);
        (self.on_change)(width, previous);
    }
}
