//! The preset list: every built-in curve with a thumbnail of what it does,
//! the person's own profiles with rename, delete and export, and the way into
//! import. Saving the curve in force as a new preset lives here too.

use super::curve::plot_accent;
use super::{copy, import, render, Screen};
use adw::prelude::*;
use flaccy_shared::equalizer::biquad::BiquadMath;
use flaccy_shared::equalizer::presets::EqualizerPresets;
use flaccy_shared::equalizer::profile::EqualizerProfile;
use gtk::{gio, glib};
use std::rc::{Rc, Weak};

const THUMBNAIL_POINTS: usize = 40;
const THUMBNAIL_SIZE: (i32, i32) = (44, 22);

/// The sections of the list in the order the screens show them.
pub fn sections(user: &[EqualizerProfile]) -> Vec<(&'static str, Vec<EqualizerProfile>)> {
    let by_keys = |keys: &[&str]| {
        keys.iter()
            .filter_map(|key| EqualizerPresets::profile_for_key(key))
            .collect::<Vec<_>>()
    };
    let mut sections = vec![("Bit-perfect", vec![EqualizerPresets::flat()])];
    if !user.is_empty() {
        sections.push(("Yours", user.to_vec()));
    }
    sections.push(("Tune", by_keys(EqualizerPresets::TUNE_KEYS)));
    sections.push(("Genres", by_keys(EqualizerPresets::GENRE_KEYS)));
    sections
}

/// The dB curve a thumbnail draws for a profile.
pub fn thumbnail_curve(profile: &EqualizerProfile) -> Vec<f64> {
    BiquadMath::curve(&profile.resolved_bands(), THUMBNAIL_POINTS)
}

fn subtitle(profile: &EqualizerProfile) -> String {
    match EqualizerPresets::key_for_id(&profile.id) {
        Some(key) => copy::preset_subtitle(key).to_string(),
        None => copy::kind_subtitle(profile.kind, profile.bands.len()),
    }
}

fn thumbnail(curve: Vec<f64>, is_current: bool) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder()
        .content_width(THUMBNAIL_SIZE.0)
        .content_height(THUMBNAIL_SIZE.1)
        .valign(gtk::Align::Center)
        .build();
    area.set_draw_func(move |_, cr, width, height| {
        let dark = adw::StyleManager::default().is_dark();
        let ink = if dark { (1.0, 1.0, 1.0) } else { (0.09, 0.09, 0.13) };
        let (color, alpha) = if is_current { (plot_accent(dark), 1.0) } else { (ink, 0.7) };
        render::draw_thumbnail(cr, width as f64, height as f64, &curve, color, alpha, false, ink);
    });
    area
}

pub fn build_popover(weak: Weak<Screen>) -> (gtk::Popover, gtk::ListBox) {
    let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
    list.add_css_class("eq-presets");
    list.update_property(&[gtk::accessible::Property::Label("Presets")]);
    {
        let weak = weak.clone();
        list.connect_row_activated(move |_, row| {
            let Some(screen) = weak.upgrade() else { return };
            let id = screen.preset_ids.borrow().get(row.index() as usize).cloned().flatten();
            if let Some(id) = id {
                screen.choose_profile(&id);
            }
        });
    }
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(380)
        .max_content_height(520)
        .propagate_natural_height(true)
        .min_content_width(360)
        .child(&list)
        .build();

    let footer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    footer.add_css_class("eq-presets-footer");
    let entries: [(&str, fn(&Rc<Screen>)); 4] = [
        ("Import from Text…", |screen| import::present(screen, None)),
        ("Export as Text…", |screen| screen.export_profile(screen.profile())),
        ("Copy as Text", |screen| screen.copy_profile(&screen.profile())),
        ("Find Headphones on AutoEq", |screen| {
            gtk::UriLauncher::new(copy::AUTOEQ_URL).launch(
                Some(&screen.window),
                None::<&gio::Cancellable>,
                |_| {},
            );
        }),
    ];
    for (title, action) in entries {
        let button = gtk::Button::with_label(title);
        button.add_css_class("flat");
        if let Some(label) = button.child().and_then(|child| child.downcast::<gtk::Label>().ok()) {
            label.set_xalign(0.0);
        }
        let weak = weak.clone();
        button.connect_clicked(move |button| {
            if let Some(screen) = weak.upgrade() {
                if let Some(popover) = button.ancestor(gtk::Popover::static_type()) {
                    popover.downcast_ref::<gtk::Popover>().map(|p| p.popdown());
                }
                action(&screen);
            }
        });
        footer.append(&button);
    }

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.set_size_request(340, -1);
    content.append(&scroll);
    content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    content.append(&footer);
    let popover = gtk::Popover::builder().child(&content).build();
    popover.add_css_class("eq-popover");
    {
        let weak = weak.clone();
        popover.connect_show(move |_| {
            if let Some(screen) = weak.upgrade() {
                rebuild(&screen);
            }
        });
    }
    (popover, list)
}

fn header_row(title: &str) -> gtk::ListBoxRow {
    let label = gtk::Label::builder().label(title).xalign(0.0).build();
    label.add_css_class("eq-caption");
    label.add_css_class("eq-section");
    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&label));
    row.set_activatable(false);
    row.set_selectable(false);
    row
}

fn profile_row(profile: &EqualizerProfile, is_current: bool) -> gtk::ListBoxRow {
    let names = gtk::Box::new(gtk::Orientation::Vertical, 1);
    names.set_hexpand(true);
    let name = gtk::Label::builder().label(&profile.name).xalign(0.0).build();
    name.add_css_class("eq-preset-name");
    let note = gtk::Label::builder().label(subtitle(profile)).xalign(0.0).build();
    note.add_css_class("eq-preset-note");
    names.append(&name);
    names.append(&note);

    let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    content.add_css_class("eq-preset-row");
    content.append(&thumbnail(thumbnail_curve(profile), is_current));
    content.append(&names);
    if is_current {
        let tick = gtk::Image::from_icon_name("object-select-symbolic");
        tick.add_css_class("eq-preset-tick");
        content.append(&tick);
    }
    if !profile.is_built_in() {
        let menu = gio::Menu::new();
        for (title, action) in [
            ("Rename…", "eqw.rename"),
            ("Export as Text…", "eqw.export"),
            ("Copy as Text", "eqw.copy"),
            ("Delete", "eqw.delete"),
        ] {
            let item = gio::MenuItem::new(Some(title), None);
            item.set_action_and_target_value(Some(action), Some(&profile.id.to_variant()));
            menu.append_item(&item);
        }
        let more = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .menu_model(&menu)
            .valign(gtk::Align::Center)
            .build();
        more.add_css_class("flat");
        more.add_css_class("circular");
        more.update_property(&[gtk::accessible::Property::Label(&format!("More for {}", profile.name))]);
        content.append(&more);
    }
    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&content));
    if is_current {
        row.add_css_class("eq-preset-current");
    }
    row.update_property(&[gtk::accessible::Property::Label(&profile.name)]);
    row
}

pub fn rebuild(screen: &Screen) {
    let list = &screen.preset_list;
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    let current = screen.profile().id;
    let mut ids = Vec::new();
    for (title, profiles) in sections(&screen.engine.state().user_profiles) {
        list.append(&header_row(title));
        ids.push(None);
        for profile in profiles {
            list.append(&profile_row(&profile, profile.id == current));
            ids.push(Some(profile.id));
        }
    }
    *screen.preset_ids.borrow_mut() = ids;
}

pub fn install_actions(screen: &Rc<Screen>) {
    let string_action = |name: &str, handler: fn(&Rc<Screen>, &str)| {
        let action = gio::SimpleAction::new(name, Some(glib::VariantTy::STRING));
        let weak = Rc::downgrade(screen);
        action.connect_activate(move |_, parameter| {
            let (Some(screen), Some(id)) = (weak.upgrade(), parameter.and_then(|p| p.get::<String>())) else {
                return;
            };
            handler(&screen, &id);
        });
        screen.actions.add_action(&action);
    };
    string_action("rename", |screen, id| screen.prompt_rename(id));
    string_action("delete", |screen, id| screen.confirm_delete(id));
    string_action("export", |screen, id| {
        if let Some(profile) = screen.engine.state().profile_with_id(id) {
            screen.export_profile(profile);
        }
    });
    string_action("copy", |screen, id| {
        if let Some(profile) = screen.engine.state().profile_with_id(id) {
            screen.copy_profile(&profile);
        }
    });
}

fn name_dialog(heading: &str, body: &str, confirm: &str, initial: &str) -> (adw::AlertDialog, gtk::Entry) {
    let dialog = adw::AlertDialog::builder()
        .heading(heading)
        .body(body)
        .close_response("cancel")
        .default_response("confirm")
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("confirm", confirm);
    dialog.set_response_appearance("confirm", adw::ResponseAppearance::Suggested);
    let entry = gtk::Entry::builder()
        .text(initial)
        .activates_default(true)
        .placeholder_text("Name")
        .build();
    dialog.set_extra_child(Some(&entry));
    (dialog, entry)
}

pub fn prompt_save(screen: &Rc<Screen>) {
    let (dialog, entry) = name_dialog(
        "Save as Preset",
        "Give this curve a name. It appears at the start of your presets.",
        "Save",
        &screen.profile().name,
    );
    let screen = Rc::clone(screen);
    let window = screen.window.clone();
    let field = entry.clone();
    dialog.connect_response(None, move |_, response| {
        let name = field.text().trim().to_string();
        if response != "confirm" || name.is_empty() {
            return;
        }
        screen.begin_gesture();
        screen.engine.save_in_force_as(&name);
        screen.finish_gesture();
        screen.refresh(false);
        screen.toast(&format!("Saved \u{201C}{name}\u{201D}"));
    });
    dialog.present(Some(&window));
    entry.grab_focus();
}

impl Screen {
    /// Picks a profile from the list: the equalizer turns on, as choosing one asks to hear it.
    pub(super) fn choose_profile(self: &Rc<Self>, id: &str) {
        self.begin_gesture();
        self.engine.select(id);
        self.finish_gesture();
        self.preset_popover.popdown();
        self.refresh(false);
    }

    pub(super) fn prompt_rename(self: &Rc<Self>, id: &str) {
        let Some(profile) = self.engine.state().profile_with_id(id) else {
            return;
        };
        let (dialog, entry) = name_dialog("Rename Preset", "Choose a new name.", "Rename", &profile.name);
        let screen = Rc::clone(self);
        let id = id.to_string();
        dialog.connect_response(None, move |_, response| {
            let name = entry.text().trim().to_string();
            if response != "confirm" || name.is_empty() {
                return;
            }
            screen.begin_gesture();
            screen.engine.rename(&id, &name);
            screen.finish_gesture();
            screen.refresh(false);
        });
        dialog.present(Some(&self.window));
        self.preset_popover.popdown();
    }

    pub(super) fn confirm_delete(self: &Rc<Self>, id: &str) {
        let Some(profile) = self.engine.state().profile_with_id(id) else {
            return;
        };
        let dialog = adw::AlertDialog::builder()
            .heading(format!("Delete \u{201C}{}\u{201D}?", profile.name))
            .body("This removes the preset. Anything that used it goes back to Flat.")
            .close_response("cancel")
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("delete", "Delete");
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        let screen = Rc::clone(self);
        let id = id.to_string();
        dialog.connect_response(None, move |_, response| {
            if response == "delete" {
                screen.begin_gesture();
                screen.engine.delete(&id);
                screen.finish_gesture();
                screen.refresh(false);
            }
        });
        dialog.present(Some(&self.window));
        self.preset_popover.popdown();
    }

    pub(super) fn copy_profile(&self, profile: &EqualizerProfile) {
        let text = self.engine.export_text(profile);
        if let Some(display) = gtk::gdk::Display::default() {
            display.clipboard().set_text(&text);
            self.toast("Copied to the clipboard");
        }
    }

    pub(super) fn export_profile(self: &Rc<Self>, profile: EqualizerProfile) {
        let text = self.engine.export_text(&profile);
        let dialog = gtk::FileDialog::builder()
            .title("Export as Text")
            .initial_name(format!("{}.txt", profile.name))
            .build();
        let screen = Rc::clone(self);
        dialog.save(Some(&self.window), None::<&gio::Cancellable>, move |result| {
            let Ok(file) = result else { return };
            let Some(path) = file.path() else { return };
            match std::fs::write(&path, text) {
                Ok(()) => screen.toast("Profile exported"),
                Err(err) => {
                    crate::logger::warn("equalizer", &format!("export failed: {err}"));
                    screen.toast("Could not write the file");
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_leads_with_flat_then_yours_then_tune_then_genres() {
        let mine = EqualizerProfile::graphic("mine", "Mine", vec![0.0; 10]);
        let sections = sections(&[mine]);
        let titles: Vec<&str> = sections.iter().map(|(title, _)| *title).collect();
        assert_eq!(titles, vec!["Bit-perfect", "Yours", "Tune", "Genres"]);
        let total: usize = sections.iter().map(|(_, profiles)| profiles.len()).sum();
        assert_eq!(total, 1 + 1 + 8 + 7);
    }

    #[test]
    fn without_profiles_of_your_own_the_section_is_left_out() {
        let titles: Vec<&str> = sections(&[]).iter().map(|(title, _)| *title).collect();
        assert_eq!(titles, vec!["Bit-perfect", "Tune", "Genres"]);
    }

    #[test]
    fn a_preset_thumbnail_has_a_point_per_sample_and_flat_stays_flat() {
        let flat = thumbnail_curve(&EqualizerPresets::flat());
        assert_eq!(flat.len(), THUMBNAIL_POINTS);
        assert!(flat.iter().all(|value| value.abs() < 1e-6));
        let rock = thumbnail_curve(&EqualizerPresets::profile_for_key("rock").unwrap());
        assert!(rock.iter().any(|value| *value > 2.0));
    }

    #[test]
    fn a_thumbnail_rises_where_the_preset_boosts() {
        let bass = thumbnail_curve(&EqualizerPresets::profile_for_key("bassBoost").unwrap());
        let points = render::thumbnail_points(&bass, 44.0, 22.0, 9.0);
        assert!(points[2].1 < points[THUMBNAIL_POINTS - 1].1);
    }

    #[test]
    fn user_profiles_describe_their_shape() {
        let simple = EqualizerProfile::graphic("a", "A", vec![0.0; 10]);
        assert_eq!(subtitle(&simple), "Simple");
        let advanced = EqualizerProfile::parametric("b", "B", Vec::new());
        assert_eq!(subtitle(&advanced), "Advanced · 0 bands");
        assert_eq!(subtitle(&EqualizerPresets::flat()), "No processing, bit-perfect");
    }
}
