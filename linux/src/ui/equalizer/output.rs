//! The output button's menu: which profile the output you are listening on
//! keeps, what the outputs you used before keep, and what everything else
//! falls back to. Mirrors the Mac's output menu item for item.

use super::{copy, Screen};
use adw::prelude::*;
use flaccy_shared::equalizer::presets::EqualizerPresets;
use flaccy_shared::equalizer::profile::EqualizerProfile;
use flaccy_shared::equalizer::state::EqualizerState;
use gtk::{gio, glib};
use std::rc::Rc;

const SEPARATOR: char = '\u{1f}';
const OFF: &str = "off";
const FORGET: &str = "forget";

/// What the output menu can do to an output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    Off,
    Profile(String),
    Forget,
}

/// The target string a menu item carries: the output and what to do with it.
pub fn encode(output_id: &str, choice: &Choice) -> String {
    let tail = match choice {
        Choice::Off => OFF.to_string(),
        Choice::Forget => FORGET.to_string(),
        Choice::Profile(id) => format!("profile:{id}"),
    };
    format!("{output_id}{SEPARATOR}{tail}")
}

pub fn decode(text: &str) -> Option<(String, Choice)> {
    let (output, tail) = text.split_once(SEPARATOR)?;
    let choice = match tail {
        OFF => Choice::Off,
        FORGET => Choice::Forget,
        other => Choice::Profile(other.strip_prefix("profile:")?.to_string()),
    };
    Some((output.to_string(), choice))
}

fn profile_submenu(
    action: &str,
    output_id: &str,
    user: &[EqualizerProfile],
    include_off: bool,
    include_forget: bool,
) -> gio::Menu {
    let menu = gio::Menu::new();
    let item = |title: &str, choice: Choice| {
        let item = gio::MenuItem::new(Some(title), None);
        item.set_action_and_target_value(Some(action), Some(&encode(output_id, &choice).to_variant()));
        item
    };
    if include_off {
        let section = gio::Menu::new();
        section.append_item(&item("Off · bit-perfect", Choice::Off));
        menu.append_section(None, &section);
    }
    if !user.is_empty() {
        let section = gio::Menu::new();
        for profile in user {
            section.append_item(&item(&profile.name, Choice::Profile(profile.id.clone())));
        }
        menu.append_section(Some("Yours"), &section);
    }
    let presets = gio::Menu::new();
    for profile in EqualizerPresets::all() {
        presets.append_item(&item(&profile.name, Choice::Profile(profile.id)));
    }
    menu.append_section(Some("Presets"), &presets);
    if include_forget {
        let section = gio::Menu::new();
        section.append_item(&item("Forget this Output", Choice::Forget));
        menu.append_section(None, &section);
    }
    menu
}

fn binding_name(state: &EqualizerState, binding: &str) -> String {
    if binding == EqualizerState::OFF_BINDING {
        "Off · bit-perfect".to_string()
    } else {
        state.profile_with_id(binding).map(|profile| profile.name).unwrap_or_default()
    }
}

/// Builds the whole menu for the current output and the outputs remembered.
pub fn build_menu(screen: &Screen) -> gio::Menu {
    let state = screen.engine.state();
    let output = screen.current_output();
    let current_id = output.as_ref().map(|output| output.id.clone());
    let root = gio::Menu::new();

    if let Some(id) = &current_id {
        let header = output.as_ref().map_or("Speaker", |output| output.name.as_str());
        let this = gio::Menu::new();
        this.append(Some("Remember for this Output"), Some("eqw.remember"));
        this.append_submenu(
            Some("Use Profile"),
            &profile_submenu("eqw.bind", id, &state.user_profiles, true, false),
        );
        root.append_section(Some(header), &this);
    }

    let others: Vec<_> = screen
        .engine
        .remembered_outputs()
        .into_iter()
        .filter(|entry| Some(&entry.id) != current_id.as_ref())
        .collect();
    if !others.is_empty() {
        let section = gio::Menu::new();
        for other in others {
            let title = format!("{} · {}", other.name, binding_name(&state, &other.binding));
            section.append_submenu(
                Some(&title),
                &profile_submenu("eqw.bind", &other.id, &state.user_profiles, true, true),
            );
        }
        root.append_section(Some("Other Outputs"), &section);
    }

    let fallback = gio::Menu::new();
    fallback.append_submenu(
        Some("Other Outputs Use"),
        &profile_submenu("eqw.default", "", &state.user_profiles, false, false),
    );
    root.append_section(None, &fallback);
    root
}

fn signature(screen: &Screen) -> String {
    let state = screen.engine.state();
    let user: Vec<String> = state.user_profiles.iter().map(|p| format!("{}={}", p.id, p.name)).collect();
    let remembered: Vec<String> = screen
        .engine
        .remembered_outputs()
        .iter()
        .map(|entry| format!("{}={}={}", entry.id, entry.name, entry.binding))
        .collect();
    format!(
        "{:?}|{}|{}",
        screen.current_output().map(|output| (output.id, output.name)),
        user.join(","),
        remembered.join(",")
    )
}

/// Rebuilds the menu only when something it lists changed, so a drag that
/// edits fifty times a second does not rebuild a hundred menu items.
pub fn rebuild_menu_if_changed(screen: &Screen) {
    let wanted = signature(screen);
    if *screen.output_signature.borrow() == wanted {
        return;
    }
    *screen.output_signature.borrow_mut() = wanted;
    screen.output_button.set_menu_model(Some(&build_menu(screen)));
}

pub fn install_actions(screen: &Rc<Screen>) {
    let weak = Rc::downgrade(screen);
    {
        let weak = weak.clone();
        screen.remember_action.connect_activate(move |_, _| {
            let Some(screen) = weak.upgrade() else { return };
            let keeps = screen.engine.keeps_own_profile_for_output();
            screen.begin_gesture();
            screen.engine.set_remembering(!keeps);
            screen.finish_gesture();
            screen.refresh(false);
        });
    }
    let bind = gio::SimpleAction::new("bind", Some(glib::VariantTy::STRING));
    {
        let weak = weak.clone();
        bind.connect_activate(move |_, parameter| {
            let Some(screen) = weak.upgrade() else { return };
            let Some((output, choice)) = parameter.and_then(|p| p.get::<String>()).and_then(|t| decode(&t)) else {
                return;
            };
            screen.begin_gesture();
            match choice {
                Choice::Off => screen.engine.bind_output_to_off(Some(&output)),
                Choice::Profile(id) => screen.engine.set_binding(&id, &output),
                Choice::Forget => screen.engine.forget_output(&output),
            }
            screen.finish_gesture();
            screen.refresh(false);
        });
    }
    screen.actions.add_action(&bind);
    let default = gio::SimpleAction::new("default", Some(glib::VariantTy::STRING));
    default.connect_activate(move |_, parameter| {
        let Some(screen) = weak.upgrade() else { return };
        if let Some((_, Choice::Profile(id))) = parameter.and_then(|p| p.get::<String>()).and_then(|t| decode(&t)) {
            screen.begin_gesture();
            screen.engine.set_default_profile(&id);
            screen.finish_gesture();
            screen.refresh(false);
            screen.toast(&format!("Other outputs use {}", copy_name(&screen, &id)));
        }
    });
    screen.actions.add_action(&default);
}

fn copy_name(screen: &Screen, id: &str) -> String {
    screen
        .engine
        .state()
        .profile_with_id(id)
        .map(|profile| profile.name)
        .unwrap_or_else(|| copy::TITLE.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_menu_target_round_trips() {
        for choice in [Choice::Off, Choice::Forget, Choice::Profile("builtin.rock".into())] {
            let text = encode("alsa_output.usb-Scarlett", &choice);
            assert_eq!(decode(&text), Some(("alsa_output.usb-Scarlett".to_string(), choice)));
        }
    }

    #[test]
    fn a_malformed_target_is_refused() {
        assert_eq!(decode("no separator"), None);
        assert_eq!(decode(&format!("sink{SEPARATOR}mystery")), None);
    }

    #[test]
    fn the_off_binding_reads_as_bit_perfect() {
        let state = EqualizerState::default();
        assert_eq!(binding_name(&state, EqualizerState::OFF_BINDING), "Off · bit-perfect");
        assert_eq!(binding_name(&state, "builtin.rock"), "Rock");
        assert_eq!(binding_name(&state, "gone"), "");
    }
}
