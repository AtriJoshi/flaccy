use gtk::prelude::*;

/// Whether anyone can be looking at `widget` right now: it is mapped, and the
/// compositor has not suspended its window. A suspended window gets no frames,
/// so a timer that keeps redrawing it would be waking the process to paint
/// nothing.
pub fn is_watched(widget: &impl IsA<gtk::Widget>) -> bool {
    let widget = widget.as_ref();
    widget.is_mapped()
        && !widget
            .root()
            .and_downcast::<gtk::Window>()
            .is_some_and(|window| window.is_suspended())
}
