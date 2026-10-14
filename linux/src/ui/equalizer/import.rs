//! The import sheet: paste or open an Equalizer APO or AutoEq profile, see the
//! curve it makes, and add it. The same text the Apple clients read, through
//! the same parser; this mirrors `MacEqualizerImportViewController`.

use super::curve::plot_accent;
use super::{copy, render, Screen};
use crate::eq::engine::ImportOutcome;
use adw::prelude::*;
use flaccy_shared::equalizer::biquad::BiquadMath;
use flaccy_shared::equalizer::graphic::GraphicSolver;
use flaccy_shared::equalizer::profile::EqualizerKind;
use flaccy_shared::equalizer::profile_text::{EqualizerProfileText, Parsed};
use gtk::gio;
use std::cell::RefCell;
use std::rc::Rc;

const FALLBACK_NAME: &str = "Imported profile";
const PREVIEW_POINTS: usize = 120;

/// How many filters a parsed profile holds, as the detected line counts them.
pub fn filter_count(parsed: &Parsed) -> usize {
    match parsed.profile.kind {
        EqualizerKind::Graphic => GraphicSolver::CENTRES.len(),
        EqualizerKind::Parametric => parsed.profile.bands.len(),
    }
}

pub fn detected_line(parsed: &Parsed) -> String {
    format!("Profile detected · {} filters", filter_count(parsed))
}

/// The name to add the profile under: what was typed, else what the text called it.
pub fn chosen_name(typed: &str, parsed: &Parsed) -> String {
    let typed = typed.trim();
    if typed.is_empty() {
        parsed.profile.name.clone()
    } else {
        typed.to_string()
    }
}

struct Sheet {
    buffer: gtk::TextBuffer,
    detected: gtk::Label,
    warning: gtk::Label,
    preview: gtk::DrawingArea,
    name: gtk::Entry,
    add: gtk::Button,
    use_it: gtk::Button,
    parsed: RefCell<Option<Parsed>>,
    curve: RefCell<Vec<f64>>,
    name_was_typed: RefCell<bool>,
}

impl Sheet {
    fn text(&self) -> String {
        let (start, end) = self.buffer.bounds();
        self.buffer.text(&start, &end, false).to_string()
    }

    fn reparse(&self) {
        let parsed = EqualizerProfileText::parse(&self.text(), FALLBACK_NAME);
        let has_profile = parsed.is_some();
        for widget in [
            self.detected.upcast_ref::<gtk::Widget>(),
            self.preview.upcast_ref(),
            self.name.upcast_ref(),
        ] {
            widget.set_visible(has_profile);
        }
        self.add.set_sensitive(has_profile);
        self.use_it.set_sensitive(has_profile);
        match &parsed {
            Some(parsed) => {
                self.detected.set_text(&detected_line(parsed));
                self.warning.set_visible(parsed.dropped_filters > 0);
                self.warning.set_text(&copy::dropped_filters_warning(parsed.dropped_filters));
                if self.name.text().is_empty() || !*self.name_was_typed.borrow() {
                    self.name.set_text(&parsed.profile.name);
                }
                *self.curve.borrow_mut() =
                    BiquadMath::curve(&parsed.profile.resolved_bands(), PREVIEW_POINTS);
            }
            None => {
                self.warning.set_visible(false);
                self.curve.borrow_mut().clear();
            }
        }
        *self.parsed.borrow_mut() = parsed;
        self.preview.queue_draw();
    }
}

/// Opens the sheet, optionally with text already in it.
pub fn present(screen: &Rc<Screen>, seed: Option<&str>) {
    let buffer = gtk::TextBuffer::new(None);
    let text_view = gtk::TextView::builder()
        .buffer(&buffer)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::None)
        .top_margin(8)
        .bottom_margin(8)
        .left_margin(10)
        .right_margin(10)
        .build();
    text_view.add_css_class("eq-import-text");
    text_view.update_property(&[gtk::accessible::Property::Label("Profile text")]);
    let scroll = gtk::ScrolledWindow::builder()
        .min_content_height(150)
        .max_content_height(210)
        .propagate_natural_height(true)
        .child(&text_view)
        .build();
    scroll.add_css_class("card");

    let paste = gtk::Button::with_label("Paste");
    let file = gtk::Button::with_label("Choose File…");
    let site = gtk::Button::with_label("autoeq.app");
    let sources = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    sources.append(&paste);
    sources.append(&file);
    sources.append(&site);

    let detected = gtk::Label::builder().xalign(0.0).build();
    detected.add_css_class("eq-status-lossless");
    detected.add_css_class("eq-detected");
    let preview = gtk::DrawingArea::builder().content_height(84).hexpand(true).build();
    preview.update_property(&[gtk::accessible::Property::Label("Preview of the imported curve")]);
    let warning = gtk::Label::builder().xalign(0.0).wrap(true).build();
    warning.add_css_class("eq-status-caution");
    let name = gtk::Entry::builder().placeholder_text("Name").activates_default(false).build();
    name.update_property(&[gtk::accessible::Property::Label("Preset name")]);

    let add = gtk::Button::with_label("Add to Presets");
    let use_it = gtk::Button::with_label("Add and Use");
    use_it.add_css_class("suggested-action");
    let cancel = gtk::Button::with_label("Cancel");
    let spacer = gtk::Box::builder().hexpand(true).build();
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.append(&cancel);
    buttons.append(&spacer);
    buttons.append(&add);
    buttons.append(&use_it);

    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(6)
        .margin_bottom(18)
        .margin_start(22)
        .margin_end(22)
        .build();
    for widget in [
        scroll.upcast_ref::<gtk::Widget>(),
        sources.upcast_ref(),
        detected.upcast_ref(),
        preview.upcast_ref(),
        warning.upcast_ref(),
        name.upcast_ref(),
        buttons.upcast_ref(),
    ] {
        column.append(widget);
    }

    let sheet = Rc::new(Sheet {
        buffer: buffer.clone(),
        detected,
        warning,
        preview: preview.clone(),
        name: name.clone(),
        add: add.clone(),
        use_it: use_it.clone(),
        parsed: RefCell::new(None),
        curve: RefCell::new(Vec::new()),
        name_was_typed: RefCell::new(false),
    });

    {
        let sheet = Rc::downgrade(&sheet);
        preview.set_draw_func(move |_, cr, width, height| {
            let Some(sheet) = sheet.upgrade() else { return };
            let dark = adw::StyleManager::default().is_dark();
            let ink = if dark { (1.0, 1.0, 1.0) } else { (0.09, 0.09, 0.13) };
            render::draw_thumbnail(
                cr,
                width as f64,
                height as f64,
                &sheet.curve.borrow(),
                plot_accent(dark),
                1.0,
                true,
                ink,
            );
        });
    }
    {
        let sheet = Rc::downgrade(&sheet);
        buffer.connect_changed(move |_| {
            if let Some(sheet) = sheet.upgrade() {
                sheet.reparse();
            }
        });
    }
    {
        let sheet = Rc::downgrade(&sheet);
        name.connect_changed(move |entry| {
            if let Some(sheet) = sheet.upgrade() {
                if entry.has_focus() || entry.focus_child().is_some() {
                    *sheet.name_was_typed.borrow_mut() = true;
                }
            }
        });
    }

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&column));
    let dialog = adw::Dialog::builder()
        .title("Import a Profile")
        .content_width(540)
        .child(&toolbar)
        .build();

    {
        let buffer = buffer.clone();
        paste.connect_clicked(move |_| {
            let Some(display) = gtk::gdk::Display::default() else { return };
            let buffer = buffer.clone();
            display.clipboard().read_text_async(None::<&gio::Cancellable>, move |result| {
                if let Ok(Some(text)) = result {
                    if !text.is_empty() {
                        buffer.set_text(&text);
                    }
                }
            });
        });
    }
    {
        let screen = Rc::clone(screen);
        let buffer = buffer.clone();
        let name = name.clone();
        let sheet = Rc::clone(&sheet);
        file.connect_clicked(move |_| {
            let dialog = gtk::FileDialog::builder().title("Choose a Profile").build();
            let buffer = buffer.clone();
            let name = name.clone();
            let sheet = Rc::clone(&sheet);
            dialog.open(Some(&screen.window), None::<&gio::Cancellable>, move |result| {
                let Ok(file) = result else { return };
                let Some(path) = file.path() else { return };
                let Ok(text) = std::fs::read_to_string(&path) else { return };
                *sheet.name_was_typed.borrow_mut() = false;
                buffer.set_text(&text);
                if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
                    name.set_text(stem);
                    *sheet.name_was_typed.borrow_mut() = true;
                }
            });
        });
    }
    {
        let screen = Rc::clone(screen);
        site.connect_clicked(move |_| {
            gtk::UriLauncher::new(copy::AUTOEQ_URL).launch(Some(&screen.window), None::<&gio::Cancellable>, |_| {});
        });
    }
    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| {
            dialog.close();
        });
    }
    for (button, use_profile) in [(&add, false), (&use_it, true)] {
        let screen = Rc::clone(screen);
        let sheet = Rc::clone(&sheet);
        let dialog = dialog.clone();
        button.connect_clicked(move |_| {
            if commit(&screen, &sheet, use_profile) {
                dialog.close();
            }
        });
    }

    sheet.reparse();
    if let Some(seed) = seed {
        buffer.set_text(seed);
    }
    dialog.present(Some(&screen.window));
    text_view.grab_focus();
}

fn commit(screen: &Rc<Screen>, sheet: &Sheet, use_profile: bool) -> bool {
    let name = {
        let parsed = sheet.parsed.borrow();
        let Some(parsed) = parsed.as_ref() else {
            return false;
        };
        chosen_name(sheet.name.text().as_str(), parsed)
    };
    screen.begin_gesture();
    let outcome = screen.engine.import_text(&sheet.text(), &name, use_profile);
    let ImportOutcome::Imported { profile, .. } = outcome else {
        screen.finish_gesture();
        return false;
    };
    if profile.name != name {
        screen.engine.rename(&profile.id, &name);
    }
    screen.finish_gesture();
    screen.refresh(false);
    screen.toast(&format!("Added \u{201C}{name}\u{201D}"));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUTOEQ: &str = "Preamp: -4.2 dB\nFilter 1: ON PK Fc 31 Hz Gain 3.1 dB Q 1.10\nFilter 2: ON PK Fc 250 Hz Gain -2.0 dB Q 0.90\nFilter 3: ON HS Fc 9000 Hz Gain 2.5 dB Q 0.70\n";

    #[test]
    fn a_pasted_profile_is_detected_with_its_filter_count() {
        let parsed = EqualizerProfileText::parse(AUTOEQ, FALLBACK_NAME).expect("parsed");
        assert_eq!(filter_count(&parsed), 3);
        assert_eq!(detected_line(&parsed), "Profile detected · 3 filters");
        assert_eq!(parsed.dropped_filters, 0);
    }

    #[test]
    fn text_without_filters_is_not_a_profile() {
        assert!(EqualizerProfileText::parse("hello there", FALLBACK_NAME).is_none());
        assert!(EqualizerProfileText::parse("", FALLBACK_NAME).is_none());
    }

    #[test]
    fn an_empty_name_falls_back_to_the_one_in_the_text() {
        let parsed = EqualizerProfileText::parse(AUTOEQ, FALLBACK_NAME).expect("parsed");
        assert_eq!(chosen_name("  ", &parsed), FALLBACK_NAME);
        assert_eq!(chosen_name(" Studio ", &parsed), "Studio");
    }

    #[test]
    fn filters_past_the_limit_are_counted_as_dropped() {
        let many: String = (1..=12)
            .map(|n| format!("Filter {n}: ON PK Fc {} Hz Gain 1.0 dB Q 1.0\n", 100 * n))
            .collect();
        let parsed = EqualizerProfileText::parse(&many, FALLBACK_NAME).expect("parsed");
        assert!(parsed.dropped_filters > 0);
        assert!(parsed.profile.bands.len() <= 10);
        assert!(copy::dropped_filters_warning(parsed.dropped_filters).contains("could not be kept"));
    }
}
