//! What the screen does when the curve, a band field or a control asks for a
//! change: one engine edit per request, and one undo step per gesture.

use super::bands::BandEvent;
use super::curve::CurveEvent;
use super::{Screen, DEFAULT_SELECTION, PREAMP_UNDO_DELAY_MS};
use adw::prelude::*;
use flaccy_shared::equalizer::biquad::FilterType;
use flaccy_shared::equalizer::interaction::PlotAction;
use flaccy_shared::equalizer::plot_model::PlotMode;
use flaccy_shared::equalizer::profile::{EqualizerBand, EqualizerKind, EqualizerProfile};
use gtk::glib;
use std::rc::Rc;
use std::time::Duration;

const SAME: f64 = 1e-9;

/// A gain held inside what a band can take.
pub fn clamp_gain(value: f64) -> f64 {
    value.clamp(EqualizerBand::GAIN_MIN, EqualizerBand::GAIN_MAX)
}

/// A Q after the wheel moved it: scaled, settled onto hundredths, kept in range.
pub fn scaled_q(q: f64, factor: f64) -> f64 {
    ((q * factor * 100.0).round() / 100.0).clamp(EqualizerBand::Q_MIN, EqualizerBand::Q_MAX)
}

/// The band to select after the selected one was removed.
pub fn selection_after_removal(index: usize) -> usize {
    index.saturating_sub(1)
}

impl Screen {
    pub(super) fn profile(&self) -> EqualizerProfile {
        self.engine.profile_in_force()
    }

    pub(super) fn mode(&self) -> PlotMode {
        Self::mode_of(&self.profile())
    }

    pub(super) fn mode_of(profile: &EqualizerProfile) -> PlotMode {
        if profile.kind == EqualizerKind::Graphic {
            PlotMode::Simple
        } else {
            PlotMode::Advanced
        }
    }

    pub(super) fn begin_gesture(&self) {
        self.undo.borrow_mut().begin(self.engine.state());
    }

    pub(super) fn finish_gesture(&self) {
        let current = self.engine.state();
        self.undo.borrow_mut().finish(&current);
    }

    /// Ends a gesture that has no release of its own, such as a slider drag,
    /// once its edits have stopped for a moment.
    fn finish_soon(self: &Rc<Self>) {
        let generation = self.finish_generation.get() + 1;
        self.finish_generation.set(generation);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(PREAMP_UNDO_DELAY_MS), move || {
            if let Some(screen) = weak.upgrade() {
                if screen.finish_generation.get() == generation {
                    screen.preamp_active.set(false);
                    screen.finish_gesture();
                }
            }
        });
    }

    pub fn undo(&self) -> bool {
        if self.undo.borrow().is_in_gesture() {
            return false;
        }
        let current = self.engine.state();
        let Some(previous) = self.undo.borrow_mut().undo(current) else {
            return false;
        };
        self.engine.restore(previous);
        self.refresh(false);
        true
    }

    pub fn redo(&self) -> bool {
        if self.undo.borrow().is_in_gesture() {
            return false;
        }
        let current = self.engine.state();
        let Some(next) = self.undo.borrow_mut().redo(current) else {
            return false;
        };
        self.engine.restore(next);
        self.refresh(false);
        true
    }

    pub(super) fn select(&self, index: usize) {
        self.selected.set(index);
        if !self.engine.state().is_enabled {
            self.engine.set_enabled(true);
        }
        self.refresh(false);
    }

    pub(super) fn enable_toggled(&self, on: bool) {
        self.begin_gesture();
        self.engine.set_enabled(on);
        self.finish_gesture();
        self.refresh(false);
    }

    pub(super) fn begin_compare(&self) {
        self.engine.begin_compare();
        self.refresh(false);
    }

    pub(super) fn end_compare(&self) {
        self.engine.end_compare();
        self.refresh(false);
    }

    pub(super) fn mode_chosen(self: &Rc<Self>, wants_advanced: bool) {
        let is_advanced = self.profile().kind == EqualizerKind::Parametric;
        if wants_advanced == is_advanced {
            return;
        }
        if wants_advanced {
            self.begin_gesture();
            self.engine.convert_in_force(EqualizerKind::Parametric);
            self.selected.set(0);
            self.finish_gesture();
            self.refresh(false);
            return;
        }
        let dialog = adw::AlertDialog::builder()
            .heading("Switch to Simple?")
            .body(
                "Simple mode holds ten points, so Flaccy matches your curve as closely as ten points allow. Your Advanced profile stays as it is.",
            )
            .close_response("cancel")
            .default_response("switch")
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("switch", "Switch");
        dialog.set_response_appearance("switch", adw::ResponseAppearance::Suggested);
        let screen = Rc::clone(self);
        dialog.connect_response(None, move |_, response| {
            if response == "switch" {
                screen.begin_gesture();
                screen.engine.convert_in_force(EqualizerKind::Graphic);
                screen.selected.set(DEFAULT_SELECTION);
                screen.finish_gesture();
            }
            screen.refresh(false);
        });
        dialog.present(Some(&self.window));
    }

    pub(super) fn preamp_moved(self: &Rc<Self>, value: f64) {
        let value = ((value * 2.0).round() / 2.0).clamp(EqualizerProfile::PREAMP_MIN, EqualizerProfile::PREAMP_MAX);
        self.preamp_active.set(true);
        self.begin_gesture();
        self.engine.edit(|profile| {
            profile.is_auto_preamp = false;
            profile.manual_preamp = value;
        });
        self.finish_soon();
    }

    pub(super) fn toggle_auto_preamp(&self) {
        self.begin_gesture();
        self.engine.edit(|profile| {
            if profile.is_auto_preamp {
                let current = profile.preamp();
                profile.manual_preamp = current;
                profile.is_auto_preamp = false;
            } else {
                profile.is_auto_preamp = true;
            }
        });
        self.finish_gesture();
        self.refresh(false);
    }

    fn edit_target(&self, index: usize, value: f64) {
        self.engine.edit(|profile| {
            if let Some(target) = profile.targets.get_mut(index) {
                *target = value;
            }
        });
    }

    fn reset_in_profile(&self, index: usize) {
        self.engine.edit(|profile| match profile.kind {
            EqualizerKind::Graphic => {
                if let Some(target) = profile.targets.get_mut(index) {
                    *target = 0.0;
                }
            }
            EqualizerKind::Parametric => {
                if let Some(band) = profile.bands.get_mut(index) {
                    band.gain = 0.0;
                }
            }
        });
    }

    pub(super) fn reset_band(&self, index: usize) {
        self.begin_gesture();
        self.reset_in_profile(index);
        self.finish_gesture();
        self.refresh(false);
    }

    pub(super) fn step_band(&self, index: usize, delta: f64) {
        self.begin_gesture();
        self.engine.edit(|profile| match profile.kind {
            EqualizerKind::Graphic => {
                if let Some(target) = profile.targets.get_mut(index) {
                    *target = clamp_gain(*target + delta);
                }
            }
            EqualizerKind::Parametric => {
                if let Some(band) = profile.bands.get_mut(index) {
                    if band.filter_type.has_gain() {
                        band.gain = clamp_gain(band.gain + delta);
                    }
                }
            }
        });
        self.finish_gesture();
        self.refresh(false);
    }

    pub(super) fn edit_band(&self, index: usize, change: impl FnOnce(&mut EqualizerBand)) {
        self.begin_gesture();
        self.engine.edit(|profile| {
            if let Some(band) = profile.bands.get_mut(index) {
                change(band);
                *band = band.clamped();
            }
        });
        self.finish_gesture();
    }

    pub(super) fn add_band(&self, frequency: f64, gain: f64) -> bool {
        if self.profile().bands.len() >= EqualizerBand::MAXIMUM_PER_PROFILE {
            return false;
        }
        self.engine.edit(|profile| {
            profile
                .bands
                .push(EqualizerBand::new(FilterType::Bell, frequency, gain, Some(1.0), true));
        });
        self.selected.set(self.profile().bands.len().saturating_sub(1));
        true
    }

    pub(super) fn delete_selected_band(&self) {
        let profile = self.profile();
        if self.mode() != PlotMode::Advanced || profile.bands.len() <= 1 {
            return;
        }
        let index = self.clamped_selection(&profile);
        self.begin_gesture();
        self.engine.edit(|profile| {
            if index < profile.bands.len() {
                profile.bands.remove(index);
            }
        });
        self.selected.set(selection_after_removal(index));
        self.finish_gesture();
        self.refresh(false);
    }

    pub(super) fn handle_curve_event(&self, event: CurveEvent) {
        match event {
            CurveEvent::Plot(action) => self.handle_plot_action(action),
            CurveEvent::ScaleQ { factor, index } => {
                self.edit_band(index, |band| band.q = scaled_q(band.q, factor));
                self.refresh(false);
            }
            CurveEvent::StepGain { delta, index } => self.step_band(index, delta),
            CurveEvent::SelectBy(offset) => {
                let profile = self.profile();
                let count = match Self::mode_of(&profile) {
                    PlotMode::Simple => flaccy_shared::equalizer::graphic::GraphicSolver::CENTRES.len(),
                    PlotMode::Advanced => profile.bands.len(),
                };
                if count > 0 {
                    let moved = (self.selected.get() as i64 + offset as i64).clamp(0, count as i64 - 1);
                    self.selected.set(moved as usize);
                    self.refresh(false);
                }
            }
            CurveEvent::ResetSelected => {
                let index = self.curve.selected_index();
                self.reset_band(index);
            }
            CurveEvent::DeleteSelected => self.delete_selected_band(),
            CurveEvent::BeginCompare => self.begin_compare(),
            CurveEvent::EndCompare => self.end_compare(),
        }
    }

    fn handle_plot_action(&self, action: PlotAction) {
        match action {
            PlotAction::BeginEditing(index) => {
                self.begin_gesture();
                self.selected.set(index);
                if !self.engine.state().is_enabled {
                    self.engine.set_enabled(true);
                }
            }
            PlotAction::SetTarget { value, index } => self.edit_target(index, value),
            PlotAction::SetFrequencyAndGain { frequency, gain, index } => {
                self.engine.edit(|profile| {
                    if let Some(band) = profile.bands.get_mut(index) {
                        band.frequency = frequency;
                        if band.filter_type.has_gain() {
                            band.gain = gain;
                        }
                    }
                });
            }
            PlotAction::ResetBand(index) => {
                self.reset_in_profile(index);
                self.finish_gesture();
            }
            PlotAction::AddBand { frequency, gain } => {
                self.begin_gesture();
                if !self.add_band(frequency, gain) {
                    self.toast("Up to 10 bands");
                }
            }
            PlotAction::Cross(_) => {}
            PlotAction::Ended => {
                self.finish_gesture();
                self.refresh(false);
                return;
            }
        }
        self.sync_curve();
    }

    pub(super) fn handle_band_event(&self, event: BandEvent) {
        match event {
            BandEvent::Select(index) => {
                if self.selected.get() != index {
                    self.select(index);
                }
                return;
            }
            BandEvent::Reset(index) => {
                self.reset_band(index);
                return;
            }
            BandEvent::Step { index, delta } => {
                self.step_band(index, delta);
                return;
            }
            BandEvent::Add => {
                self.begin_gesture();
                if !self.add_band(1_000.0, 0.0) {
                    self.toast("Up to 10 bands");
                }
                self.finish_gesture();
            }
            BandEvent::Remove => {
                self.delete_selected_band();
                return;
            }
            BandEvent::SetTarget { index, value } => {
                let value = clamp_gain(value);
                let current = self.profile().targets.get(index).copied();
                if current.is_some_and(|current| (current - value).abs() > SAME) {
                    self.begin_gesture();
                    self.edit_target(index, value);
                    self.finish_gesture();
                }
            }
            BandEvent::Enable { index, on } => {
                if self.band_at(index).is_some_and(|band| band.is_enabled != on) {
                    self.edit_band(index, |band| band.is_enabled = on);
                }
            }
            BandEvent::SetType { index, filter_type } => {
                if self.band_at(index).is_some_and(|band| band.filter_type != filter_type) {
                    self.edit_band(index, |band| {
                        band.filter_type = filter_type;
                        band.q = filter_type.default_q();
                        if !filter_type.has_gain() {
                            band.gain = 0.0;
                        }
                    });
                }
            }
            BandEvent::SetFrequency { index, hertz } => {
                let hertz = hertz.clamp(EqualizerBand::FREQUENCY_MIN, EqualizerBand::FREQUENCY_MAX);
                if self.band_at(index).is_some_and(|band| (band.frequency - hertz).abs() > SAME) {
                    self.edit_band(index, |band| band.frequency = hertz);
                }
            }
            BandEvent::SetGain { index, decibels } => {
                let decibels = clamp_gain(decibels);
                if self.band_at(index).is_some_and(|band| (band.gain - decibels).abs() > SAME) {
                    self.edit_band(index, |band| band.gain = decibels);
                }
            }
            BandEvent::SetQ { index, q } => {
                let q = q.clamp(EqualizerBand::Q_MIN, EqualizerBand::Q_MAX);
                if self.band_at(index).is_some_and(|band| (band.q - q).abs() > SAME) {
                    self.edit_band(index, |band| band.q = q);
                }
            }
        }
        self.refresh(false);
    }

    fn band_at(&self, index: usize) -> Option<EqualizerBand> {
        self.profile().bands.get(index).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_gains_are_held_inside_the_range() {
        assert_eq!(clamp_gain(30.0), 12.0);
        assert_eq!(clamp_gain(-30.0), -12.0);
        assert_eq!(clamp_gain(3.5), 3.5);
    }

    #[test]
    fn the_wheel_scales_q_in_hundredths_and_stays_in_range() {
        assert_eq!(scaled_q(1.0, 2f64.powf(1.0 / 6.0)), 1.12);
        assert_eq!(scaled_q(9.9, 2.0), 10.0);
        assert_eq!(scaled_q(0.31, 0.5), 0.3);
    }

    #[test]
    fn removing_a_band_selects_the_one_before_it() {
        assert_eq!(selection_after_removal(0), 0);
        assert_eq!(selection_after_removal(4), 3);
    }
}
