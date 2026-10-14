//! Demo and verification hooks for the equalizer screen, driven from the
//! environment the way the other `FLACCY_DEMO_*` switches are:
//!
//! - `FLACCY_DEMO_EQ_UI=simple|advanced|presets|import|outputs` opens the
//!   window in that state with a seeded profile, a made-up output and a
//!   synthetic spectrum, so a headless capture has something to show.
//! - `FLACCY_DEMO_EQ_UI_DRIVE=1` replays a drag, a keyboard step, an undo, a
//!   compare press, a typed value, a band add and a Q scroll through the same
//!   handlers the real gestures use, and logs PASS or FAIL for each.
//!
//! The synthetic signal is the Linux twin of `EqualizerDemoSignal.swift`: the
//! spectrum of an imaginary record run through the curve in force, so the
//! picture still moves with every edit.

use super::bands::BandEvent;
use super::output::{encode, Choice};
use super::{copy, curve, import, screen, Screen};
use crate::eq::engine::Meter;
use crate::eq::output::{Output, OutputKind};
use crate::eq::EqEngine;
use crate::ui::Ui;
use adw::prelude::*;
use flaccy_shared::equalizer::biquad::BiquadMath;
use flaccy_shared::equalizer::plot_model::{PlotMode, Point};
use flaccy_shared::equalizer::profile::EqualizerBand;
use flaccy_shared::equalizer::spectrum::SpectrumAnalyzer;
use gtk::{gdk, glib};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

static ACTIVE: AtomicBool = AtomicBool::new(false);
static EPOCH: OnceLock<Instant> = OnceLock::new();

const WARM_DETAIL: &str = "# Flaccy Equalizer: Warm Detail
Preamp: -2.9 dB
Filter 1: ON HPQ Fc 28 Hz Q 0.71
Filter 2: ON LSC Fc 95 Hz Gain 3.5 dB Q 0.71
Filter 3: ON PK Fc 260 Hz Gain -2.2 dB Q 1.10
Filter 4: ON PK Fc 3300 Hz Gain 2.6 dB Q 1.40
Filter 5: ON PK Fc 6800 Hz Gain -3.2 dB Q 4.20
Filter 6: ON HSC Fc 12000 Hz Gain 2.4 dB Q 0.71
";

const STUDIO_REFERENCE: &str = "Preamp: -4.2 dB
Filter 1: ON LSC Fc 105 Hz Gain 4.5 dB Q 0.70
Filter 2: ON PK Fc 210 Hz Gain -2.1 dB Q 0.90
Filter 3: ON PK Fc 1800 Hz Gain 2.4 dB Q 1.40
Filter 4: ON PK Fc 5200 Hz Gain -3.6 dB Q 3.20
Filter 5: ON HSC Fc 10000 Hz Gain 3.1 dB Q 0.70
";

pub fn set_active(on: bool) {
    ACTIVE.store(on, Ordering::Relaxed);
}

pub fn is_active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

fn seconds() -> f64 {
    EPOCH.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// The spectrum of an imaginary record at `time`, run through `bands` and
/// `preamp_decibels`, as one bin per analyzer bin.
pub fn spectrum(time: f64, bands: &[EqualizerBand], preamp_decibels: f64) -> Vec<f32> {
    let reference = BiquadMath::REFERENCE_SAMPLE_RATE;
    let compiled: Vec<_> = bands
        .iter()
        .filter(|band| band.is_enabled)
        .map(|band| BiquadMath::coefficients_for(band, reference))
        .collect();
    let low = EqualizerBand::FREQUENCY_MIN;
    let high = EqualizerBand::FREQUENCY_MAX;
    let count = SpectrumAnalyzer::BIN_COUNT;
    (0..count)
        .map(|index| {
            let frequency = low * (high / low).powf((index as f64 + 0.5) / count as f64);
            let tilt = -4.5 * (frequency / 1_000.0).log2();
            let octave = frequency.log2();
            let wobble = 3.2 * (octave * 2.3 + time * 1.7).sin()
                + 2.2 * (octave * 5.1 - time * 2.9).sin()
                + 1.4 * (octave * 9.7 + time * 4.3).sin();
            let beat = (time * 7.2).sin().max(0.0).powi(3);
            let kick = if frequency < 160.0 {
                9.0 * beat * (-((frequency / 60.0).log2()).powi(2)).exp()
            } else {
                0.0
            };
            let air = if frequency > 12_000.0 { -10.0 * (frequency - 12_000.0) / 8_000.0 } else { 0.0 };
            let response: f64 = compiled
                .iter()
                .map(|c| BiquadMath::magnitude_decibels(*c, frequency, reference))
                .sum();
            let level = -38.0 + tilt + wobble + kick + air + response + preamp_decibels;
            level.clamp(-96.0, -6.0) as f32
        })
        .collect()
}

pub fn spectrum_for(engine: &EqEngine) -> Vec<f32> {
    let running = engine.running_profile();
    let (bands, preamp) = match &running {
        Some(profile) => (profile.resolved_bands(), profile.preamp()),
        None => (Vec::new(), 0.0),
    };
    spectrum(seconds(), &bands, preamp)
}

pub fn meter() -> Meter {
    Meter {
        peak: (0.58 + 0.06 * (seconds() * 2.1).sin()) as f32,
        guard_reduced: false,
    }
}

fn demo_output(kind: OutputKind) -> Output {
    match kind {
        OutputKind::Headphones => Output {
            id: "Studio Headphones".into(),
            name: "Studio Headphones".into(),
            kind,
        },
        _ => Output {
            id: "Desk Speakers".into(),
            name: "Desk Speakers".into(),
            kind: OutputKind::Speaker,
        },
    }
}

fn seed(screen: &Rc<Screen>, mode: &str) {
    let engine = &screen.engine;
    match mode {
        "advanced" => {
            engine.import_text(WARM_DETAIL, "Warm Detail", true);
            screen.selected.set(5);
            *screen.demo_output.borrow_mut() = Some(demo_output(OutputKind::Headphones));
        }
        _ => {
            engine.select("builtin.rock");
            screen.selected.set(5);
            *screen.demo_output.borrow_mut() = Some(demo_output(OutputKind::Speaker));
        }
    }
    engine.set_enabled(true);
    if mode == "outputs" {
        engine.set_binding("builtin.vocal", "Studio Headphones");
        engine.set_binding("builtin.classical", "Desk Speakers");
    }
}

/// Opens the equalizer window in the state `mode` names.
pub fn open(ui: &Rc<Ui>, mode: &str) {
    ACTIVE.store(true, Ordering::Relaxed);
    EPOCH.get_or_init(Instant::now);
    if let Some(theme) = crate::theme::ThemeController::current() {
        theme.set_artwork_color(Some((232, 130, 76)));
    }
    let screen = screen(ui);
    seed(&screen, mode);
    screen.refresh(false);
    if mode == "capsule" {
        return;
    }
    super::present(ui);
    let mode = mode.to_string();
    glib::timeout_add_local_once(Duration::from_millis(900), move || match mode.as_str() {
        "presets" => screen.preset_button.popup(),
        "outputs" => screen.output_button.popup(),
        "import" => import::present(&screen, Some(STUDIO_REFERENCE)),
        _ => {}
    });
}

fn report(expectation: &str, passed: bool) {
    crate::logger::info(
        "equalizer",
        &format!("EqualizerUIExercise {}: {expectation}", if passed { "PASS" } else { "FAIL" }),
    );
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

async fn pause(milliseconds: u64) {
    glib::timeout_future(Duration::from_millis(milliseconds)).await;
}

fn target(screen: &Screen, index: usize) -> f64 {
    screen.engine.profile_in_force().targets.get(index).copied().unwrap_or(f64::NAN)
}

fn centre_point(screen: &Screen, index: usize, decibels: f64) -> Point {
    screen
        .curve
        .point_at(flaccy_shared::equalizer::graphic::GraphicSolver::CENTRES[index], decibels)
}

/// Runs the scripted interaction check and logs a PASS or FAIL line per step.
pub fn drive(ui: &Rc<Ui>) {
    let ui = Rc::clone(ui);
    glib::spawn_future_local(async move {
        open(&ui, "simple");
        pause(1600).await;
        let screen = screen(&ui);
        let engine = Rc::clone(&screen.engine);
        screen.curve.area.grab_focus();
        pause(200).await;

        let before = engine.state();
        report(
            "the exercise starts on Rock with the equalizer on",
            engine.profile_in_force().id == "builtin.rock" && engine.state().is_enabled && target(&screen, 5) == 0.0,
        );

        screen.curve.press(centre_point(&screen, 5, 6.0), 10.0);
        screen.curve.drag(centre_point(&screen, 5, 8.0));
        screen.curve.drag(centre_point(&screen, 5, 7.5));
        screen.curve.release();
        report("a drag settles the 1 kHz band on +7.5 dB", near(target(&screen, 5), 7.5));
        report(
            "editing a built-in preset copies it to Custom",
            engine.profile_in_force().name == "Custom" && !engine.profile_in_force().is_built_in(),
        );

        let handled = screen.curve.key_pressed(gdk::Key::Up, gdk::ModifierType::empty());
        report("the Up key raises the selected band by 0.5 dB", handled && near(target(&screen, 5), 8.0));
        screen.curve.key_pressed(gdk::Key::Down, gdk::ModifierType::SHIFT_MASK);
        report("Shift+Down lowers it by 2 dB", near(target(&screen, 5), 6.0));
        screen.curve.key_pressed(gdk::Key::Right, gdk::ModifierType::empty());
        report("the Right key selects the next band", screen.curve.selected_index() == 6);
        screen.curve.key_pressed(gdk::Key::Left, gdk::ModifierType::empty());

        report("Ctrl+Z undoes the keyboard step", screen.window_key_pressed(gdk::Key::z, gdk::ModifierType::CONTROL_MASK) && near(target(&screen, 5), 8.0));
        screen.undo();
        report("the next undo undoes the Up step", near(target(&screen, 5), 7.5));
        screen.undo();
        report(
            "the whole drag was a single undo step and the state is exactly as it began",
            engine.state() == before && target(&screen, 5) == 0.0,
        );
        screen.window_key_pressed(gdk::Key::z, gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK);
        report("Ctrl+Shift+Z redoes the drag", near(target(&screen, 5), 7.5));
        screen.undo();

        engine.select("builtin.rock");
        screen.begin_compare();
        report(
            "pressing Hold to compare starts a comparison",
            engine.is_comparing() && screen.compare_button.label().as_deref() == Some("Hearing original"),
        );
        screen.end_compare();
        report("releasing it ends the comparison", !engine.is_comparing());
        screen.curve.area.grab_focus();
        let space = screen.window_key_pressed(gdk::Key::space, gdk::ModifierType::empty());
        report("holding Space compares", space && engine.is_comparing());
        screen.window_key_released(gdk::Key::space);
        report("releasing Space ends it", !engine.is_comparing());

        screen.band_event(BandEvent::SetTarget {
            index: 3,
            value: copy::parse_number("+3.5").unwrap_or(f64::NAN),
        });
        report("a typed +3.5 sets the 250 Hz band", near(target(&screen, 3), 3.5));
        screen.band_event(BandEvent::SetTarget { index: 3, value: 40.0 });
        report("a typed value past the range is held at +12 dB", near(target(&screen, 3), 12.0));
        screen.undo();
        screen.undo();

        screen.mode_chosen(true);
        pause(150).await;
        report(
            "choosing Advanced converts the profile to its ten filters",
            screen.mode() == PlotMode::Advanced && engine.profile_in_force().bands.len() == 10,
        );
        engine.import_text(WARM_DETAIL, "Warm Detail", true);
        screen.selected.set(0);
        screen.refresh(false);
        pause(150).await;
        let bands_before = engine.profile_in_force().bands.len();
        report("an imported profile of six filters is in force", bands_before == 6);

        let empty = screen.curve.point_at(5_000.0, -9.0);
        screen.curve.press(empty, 20.0);
        screen.curve.release();
        screen.curve.press(empty, 20.1);
        screen.curve.release();
        report(
            "a double press on empty space adds a band",
            engine.profile_in_force().bands.len() == bands_before + 1,
        );

        let added = engine.profile_in_force().bands.len() - 1;
        let q_before = engine.profile_in_force().bands[added].q;
        screen.handle_curve_event(curve::CurveEvent::ScaleQ { factor: 2.0, index: added });
        let q_after = engine.profile_in_force().bands[added].q;
        report("scrolling over a node scales its Q", near(q_after, (q_before * 2.0 * 100.0).round() / 100.0));

        let gain_before = engine.profile_in_force().bands[added].gain;
        let node = screen.curve.point_at(engine.profile_in_force().bands[added].frequency, gain_before);
        screen.curve.press(node, 30.0);
        screen.curve.release();
        screen.curve.press(node, 30.1);
        screen.curve.release();
        report("a double press on a node resets its gain", near(engine.profile_in_force().bands[added].gain, 0.0));

        screen.band_event(BandEvent::SetFrequency {
            index: 0,
            hertz: copy::parse_frequency("1.2k").unwrap_or(f64::NAN),
        });
        report("a typed 1.2k sets a band to 1200 Hz", near(engine.profile_in_force().bands[0].frequency, 1200.0));

        let output = "Desk Speakers";
        let bind = |choice: Choice| {
            screen
                .actions
                .activate_action("bind", Some(&encode(output, &choice).to_variant()));
        };
        bind(Choice::Profile("builtin.vocal".to_string()));
        report(
            "the output menu binds a profile to the output",
            engine.state().output_bindings.get(output).map(String::as_str) == Some("builtin.vocal"),
        );
        bind(Choice::Off);
        report(
            "the output menu can turn the equalizer off for the output",
            engine.state().output_bindings.get(output).map(String::as_str) == Some("off"),
        );
        bind(Choice::Forget);
        report(
            "the output menu forgets the output",
            !engine.state().output_bindings.contains_key(output),
        );
        screen.actions.activate_action("default", Some(&encode("", &Choice::Profile("builtin.jazz".to_string())).to_variant()));
        report(
            "the output menu sets the default profile",
            engine.state().active_profile_id == "builtin.jazz",
        );
        report("the curve widget takes keyboard focus", screen.curve.area.is_focusable());

        set_active(false);
        pause(3500).await;
        report("with no sound arriving the curve lets go of the frame clock", screen.curve.is_parked());
        set_active(true);
        pause(700).await;
        report("sound arriving brings the frame clock back", !screen.curve.is_parked());

        screen.window.set_visible(false);
        pause(200).await;
        let hidden = !screen.window.is_visible();
        if let Some(application) = ui.window.application() {
            application.activate_action("equalizer", None);
        }
        pause(400).await;
        report(
            "the equalizer action shows a hidden window",
            hidden && screen.window.is_visible(),
        );
        let application = ui.window.application();
        ui.window.close();
        report(
            "closing the player takes the equalizer window with it so the application can quit",
            application.is_some_and(|application| application.windows().is_empty()),
        );
        crate::logger::info("equalizer", "EqualizerUIExercise DONE");
    });
}
