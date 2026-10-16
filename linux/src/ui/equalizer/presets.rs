//! What the deck lists and what can be done to a profile: the sections in the
//! order the screens show them, a thumbnail curve for each, and rename, delete,
//! export and copy for the person's own. Saving the curve in force as a new
//! preset lives here too.

use super::{copy, Screen};
use adw::prelude::*;
use flaccy_shared::equalizer::biquad::BiquadMath;
use flaccy_shared::equalizer::presets::EqualizerPresets;
use flaccy_shared::equalizer::profile::EqualizerProfile;
use gtk::{gio, glib};
use std::rc::Rc;

const THUMBNAIL_POINTS: usize = 40;

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

pub(super) fn subtitle(profile: &EqualizerProfile) -> String {
    match EqualizerPresets::key_for_id(&profile.id) {
        Some(key) => copy::preset_subtitle(key).to_string(),
        None => copy::kind_subtitle(profile.kind, profile.bands.len()),
    }
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
    let plain_action = |name: &str, handler: fn(&Rc<Screen>)| {
        let action = gio::SimpleAction::new(name, None);
        let weak = Rc::downgrade(screen);
        action.connect_activate(move |_, _| {
            if let Some(screen) = weak.upgrade() {
                handler(&screen);
            }
        });
        screen.actions.add_action(&action);
    };
    plain_action("export-current", |screen| screen.export_profile(screen.profile()));
    plain_action("copy-current", |screen| screen.copy_profile(&screen.profile()));
    plain_action("autoeq", |screen| {
        gtk::UriLauncher::new(copy::AUTOEQ_URL).launch(Some(&screen.window), None::<&gio::Cancellable>, |_| {});
    });
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
    use super::super::render;
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
