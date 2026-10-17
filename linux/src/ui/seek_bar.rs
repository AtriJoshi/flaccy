use crate::events::AppEvent;
use crate::library::format_time;
use crate::ui::slider::{Slider, SliderMetrics, Step};
use crate::ui::screen::is_watched;
use crate::ui::Ui;
use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

/// How far the playhead may run ahead of the last position the player
/// reported. Ticks land four times a second, so a live pipeline never gets
/// this far ahead — and a stalled one freezes the knob rather than letting it
/// sail on without the audio.
const EXTRAPOLATION_LIMIT: f64 = 0.3;
/// Seconds an arrow key or wheel notch moves playback, and Page Up/Down.
const FINE_STEP: f64 = 5.0;
const COARSE_STEP: f64 = 15.0;
/// The playhead never sleeps longer than this, so a re-anchored position is
/// shown within a player tick, and never wakes more often than this, whatever
/// the track length and bar width work out to.
const LONGEST_WAKEUP: f64 = 0.25;
const SHORTEST_WAKEUP: f64 = 0.008;
/// Landing this far past a boundary makes sure the value really crossed it.
const BOUNDARY_SLACK: f64 = 0.001;

/// Elapsed time, the scrubber, and the time remaining, as one row. `bubble`
/// is the scrubber's time readout; the host adds it to an overlay covering
/// the space above the bar.
pub struct SeekBar {
    pub container: gtk::Box,
    pub bubble: gtk::Label,
}

struct State {
    ui: Rc<Ui>,
    slider: Slider,
    elapsed: gtk::Label,
    remaining: gtk::Label,
    duration: Cell<f64>,
    /// The last position the player reported and the monotonic time, in
    /// microseconds, at which it was true.
    anchor: Cell<(f64, i64)>,
    playing: Cell<bool>,
    /// The whole seconds the labels last showed, so they are only rewritten
    /// when a digit actually changes.
    shown: Cell<Option<(i64, i64, i64)>>,
    /// The pending wake-up that moves the playhead, present only while a song
    /// is playing in a bar someone can see.
    wakeup: RefCell<Option<glib::SourceId>>,
}

pub fn build(ui: &Rc<Ui>, metrics: SliderMetrics) -> SeekBar {
    let elapsed = time_label("0:00", 1.0);
    let remaining = time_label("-0:00", 0.0);
    let slider = Slider::new(metrics, "Playback position");
    slider.set_ghost(true);
    slider.area().set_hexpand(true);

    let container = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    container.add_css_class("seek-bar");
    container.append(&elapsed);
    container.append(slider.area());
    container.append(&remaining);

    let state = Rc::new(State {
        ui: Rc::clone(ui),
        slider: slider.clone(),
        elapsed,
        remaining,
        duration: Cell::new(0.0),
        anchor: Cell::new((0.0, glib::monotonic_time())),
        playing: Cell::new(false),
        shown: Cell::new(None),
        wakeup: RefCell::new(None),
    });
    state.sync_with_player();
    wire_slider(&state);
    wire_events(&state, &container);

    SeekBar {
        container,
        bubble: slider.bubble().clone(),
    }
}

fn time_label(text: &str, xalign: f32) -> gtk::Label {
    let label = gtk::Label::builder().label(text).xalign(xalign).build();
    label.add_css_class("time-label");
    label
}

/// The countdown on the right of every seek bar, in the same `-m:ss` form the
/// iPhone and Mac players use.
fn remaining_text(duration: f64, position: f64) -> String {
    format!("-{}", format_time((duration - position).max(0.0)))
}

/// Where extrapolated playback stands at `now`, given the last reported
/// position and when it was true — never past the cap, the track's end, or
/// behind zero.
fn extrapolate(anchor: (f64, i64), now: i64, playing: bool, duration: f64) -> f64 {
    let (position, since) = anchor;
    let ahead = if playing {
        ((now - since) as f64 / 1_000_000.0).clamp(0.0, EXTRAPOLATION_LIMIT)
    } else {
        0.0
    };
    let position = position + ahead;
    if duration > 0.0 {
        position.clamp(0.0, duration)
    } else {
        position.max(0.0)
    }
}

/// How long until the playhead next needs attention: when it will have moved
/// far enough to repaint (`repaint_seconds`), or when the elapsed or the
/// remaining label's whole second will turn over, whichever comes first.
fn next_wakeup(position: f64, duration: f64, repaint_seconds: f64) -> Duration {
    let remaining = (duration - position).max(0.0);
    let soonest = repaint_seconds
        .min(until_round_rises(position))
        .min(until_round_falls(remaining))
        + BOUNDARY_SLACK;
    Duration::from_secs_f64(soonest.clamp(SHORTEST_WAKEUP, LONGEST_WAKEUP))
}

/// Seconds until a value that is rising one second per second next changes
/// when rounded to a whole second.
fn until_round_rises(value: f64) -> f64 {
    let boundary = value.floor() + 0.5;
    if value < boundary {
        boundary - value
    } else {
        boundary + 1.0 - value
    }
}

/// Seconds until a value that is falling one second per second next changes
/// when rounded to a whole second.
fn until_round_falls(value: f64) -> f64 {
    let boundary = (value - 0.5 - 1e-9).floor() + 0.5;
    value - boundary
}

/// The bubble's text for a spot on the bar: its time, under the lyric line
/// sung there when the song has synced lyrics.
fn caption(ui: &Ui, seconds: f64) -> String {
    let time = glib::markup_escape_text(&format_time(seconds));
    match ui.core.lyric_line_at(seconds) {
        Some(line) => format!(
            "<span alpha=\"80%\">{}</span>\n<b>{time}</b>",
            glib::markup_escape_text(&line)
        ),
        None => format!("<b>{time}</b>"),
    }
}

impl State {
    fn sync_with_player(&self) {
        let player = &self.ui.core.player;
        let track = player.current_track();
        self.set_duration(track.as_ref().map(|track| track.duration).unwrap_or(0.0));
        self.anchor_at(player.position().unwrap_or(0.0));
        self.playing
            .set(track.is_some() && player.is_playing());
        self.render(glib::monotonic_time());
    }

    fn set_duration(&self, duration: f64) {
        let duration = if duration.is_finite() { duration.max(0.0) } else { 0.0 };
        if self.duration.replace(duration) == duration {
            return;
        }
        let digits = format_time(duration).chars().count() as i32;
        self.elapsed.set_width_chars(digits);
        self.remaining.set_width_chars(digits + 1);
        self.slider.set_enabled(duration > 0.0);
        self.slider.refresh_bubble();
    }

    fn anchor_at(&self, position: f64) {
        self.anchor.set((position, glib::monotonic_time()));
    }

    fn position_at(&self, now: i64) -> f64 {
        extrapolate(
            self.anchor.get(),
            now,
            self.playing.get(),
            self.duration.get(),
        )
    }

    fn render(&self, now: i64) {
        let duration = self.duration.get();
        let position = match self.slider.held_fraction() {
            Some(fraction) => fraction * duration,
            None => {
                let position = self.position_at(now);
                self.slider.set_value(if duration > 0.0 {
                    position / duration
                } else {
                    0.0
                });
                position
            }
        };
        self.show_times(position);
    }

    fn show_times(&self, position: f64) {
        let duration = self.duration.get();
        let key = (
            position.round() as i64,
            (duration - position).max(0.0).round() as i64,
            duration.round() as i64,
        );
        if self.shown.replace(Some(key)) == Some(key) {
            return;
        }
        self.elapsed.set_label(&format_time(position));
        self.remaining.set_label(&remaining_text(duration, position));
        self.slider.set_accessible_value(
            duration,
            position,
            &format!("{} of {}", format_time(position), format_time(duration)),
        );
    }

    /// Re-times the pending wake-up after something changed what it was
    /// computed from: the song, the play state, or the reported position.
    fn restart_playhead(self: &Rc<Self>) {
        self.disarm();
        self.sync_playhead();
    }

    fn wants_playhead(&self) -> bool {
        self.playing.get() && is_watched(self.slider.area()) && self.duration.get() > 0.0
    }

    /// Keeps a wake-up pending only while there is a playhead to move and
    /// someone to see it. The bar is repainted when the head has travelled far
    /// enough to show and when a label's second turns over, not on every
    /// vertical blank: a frame callback would hold the whole window's frame
    /// clock running for as long as a song plays.
    fn sync_playhead(self: &Rc<Self>) {
        if !self.wants_playhead() {
            self.disarm();
            return;
        }
        if self.wakeup.borrow().is_none() {
            self.advance();
        }
    }

    fn disarm(&self) {
        if let Some(id) = self.wakeup.borrow_mut().take() {
            id.remove();
        }
    }

    /// Draws the head where playback is now and sleeps until it next moves.
    fn advance(self: &Rc<Self>) {
        self.disarm();
        let now = glib::monotonic_time();
        self.render(now);
        if !self.wants_playhead() {
            return;
        }
        let duration = self.duration.get();
        let delay = next_wakeup(
            self.position_at(now),
            duration,
            self.slider.repaint_seconds(duration),
        );
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(delay, move || {
            let Some(state) = weak.upgrade() else {
                return;
            };
            state.wakeup.borrow_mut().take();
            state.advance();
        });
        *self.wakeup.borrow_mut() = Some(id);
    }

    fn seconds(&self, fraction: f64) -> f64 {
        fraction * self.duration.get()
    }
}

fn with_state(weak: &Weak<State>, apply: impl FnOnce(&Rc<State>)) {
    if let Some(state) = weak.upgrade() {
        apply(&state);
    }
}

/// A hold starts a scrub the audio chases while it moves; the release is one
/// exact seek. The player spaces the chase out — every motion event used to
/// be a flushing seek, which is what wedged the audio stream.
fn wire_slider(state: &Rc<State>) {
    let slider = &state.slider;
    {
        let weak = Rc::downgrade(state);
        slider.connect_hold(move |fraction| {
            with_state(&weak, |state| {
                let player = &state.ui.core.player;
                player.begin_scrub();
                player.scrub_to(state.seconds(fraction));
                state.render(glib::monotonic_time());
            })
        });
    }
    {
        let weak = Rc::downgrade(state);
        slider.connect_move(move |fraction| {
            with_state(&weak, |state| {
                state.ui.core.player.scrub_to(state.seconds(fraction));
                state.render(glib::monotonic_time());
            })
        });
    }
    {
        let weak = Rc::downgrade(state);
        slider.connect_release(move |fraction| {
            with_state(&weak, |state| {
                state.ui.core.player.finish_scrub(state.seconds(fraction));
                state.render(glib::monotonic_time());
            })
        });
    }
    {
        let weak = Rc::downgrade(state);
        slider.connect_cancel(move || {
            with_state(&weak, |state| {
                state.ui.core.player.abandon_scrub();
                state.render(glib::monotonic_time());
            })
        });
    }
    {
        let weak = Rc::downgrade(state);
        slider.connect_step(move |step| {
            with_state(&weak, |state| {
                let duration = state.duration.get();
                if duration <= 0.0 {
                    return;
                }
                let current = state
                    .ui
                    .core
                    .player
                    .position()
                    .unwrap_or_else(|| state.position_at(glib::monotonic_time()));
                let target = match step {
                    Step::Fine(steps) => current + steps as f64 * FINE_STEP,
                    Step::Coarse(steps) => current + steps as f64 * COARSE_STEP,
                    Step::To(fraction) => fraction * duration,
                };
                state.ui.core.player.seek(target);
            })
        });
    }
    {
        let weak = Rc::downgrade(state);
        slider.set_caption(move |fraction| {
            weak.upgrade()
                .map(|state| caption(&state.ui, state.seconds(fraction)))
                .unwrap_or_default()
        });
    }
    {
        let weak = Rc::downgrade(state);
        slider
            .area()
            .connect_map(move |_| with_state(&weak, |state| state.sync_playhead()));
    }
    {
        let weak = Rc::downgrade(state);
        slider
            .area()
            .connect_unmap(move |_| with_state(&weak, |state| state.sync_playhead()));
    }
}

fn wire_events(state: &Rc<State>, container: &gtk::Box) {
    let state = Rc::clone(state);
    let hub = Rc::clone(&state.ui.core.hub);
    hub.subscribe_widget(container, move |_, event| {
        let now = glib::monotonic_time();
        match event {
            AppEvent::TrackChanged(track) => {
                state.slider.abort_hold();
                state.set_duration(track.as_ref().map(|track| track.duration).unwrap_or(0.0));
                state.anchor_at(0.0);
                state
                    .playing
                    .set(track.is_some() && state.ui.core.player.is_playing());
                state.render(now);
                state.restart_playhead();
            }
            AppEvent::PlayingChanged(playing) => {
                state.anchor_at(state.position_at(now));
                state.playing.set(*playing);
                state.render(now);
                state.restart_playhead();
            }
            AppEvent::Tick { position, duration } => {
                if *duration > 0.0 {
                    state.set_duration(*duration);
                }
                state.anchor_at(*position);
                if state.playing.get() {
                    state.restart_playhead();
                } else {
                    state.render(now);
                }
            }
            AppEvent::Seeked(position) => {
                state.anchor_at(*position);
                state.render(now);
            }
            AppEvent::WindowSuspended(_) => state.restart_playhead(),
            _ => {}
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_down_what_is_left() {
        assert_eq!(remaining_text(225.0, 0.0), "-3:45");
        assert_eq!(remaining_text(225.0, 100.2), "-2:05");
        assert_eq!(remaining_text(225.0, 400.0), "-0:00");
        assert_eq!(remaining_text(0.0, 0.0), "-0:00");
    }

    #[test]
    fn a_playing_clock_runs_ahead_of_its_last_report() {
        let anchor = (10.0, 1_000_000);
        assert!((extrapolate(anchor, 1_150_000, true, 200.0) - 10.15).abs() < 1e-9);
    }

    #[test]
    fn a_stalled_report_freezes_the_playhead_at_the_cap() {
        let anchor = (10.0, 1_000_000);
        let later = 1_000_000 + 5_000_000;
        assert!((extrapolate(anchor, later, true, 200.0) - (10.0 + EXTRAPOLATION_LIMIT)).abs() < 1e-9);
    }

    #[test]
    fn a_paused_clock_holds_still() {
        let anchor = (42.0, 1_000_000);
        assert_eq!(extrapolate(anchor, 9_000_000, false, 200.0), 42.0);
    }

    #[test]
    fn a_label_turning_over_wakes_the_bar_before_the_head_needs_it() {
        let wake = next_wakeup(10.2, 240.0, 5.0);
        assert_eq!(wake.as_secs_f64(), LONGEST_WAKEUP);
        let wake = next_wakeup(10.45, 240.0, 5.0);
        assert!((wake.as_secs_f64() - 0.051).abs() < 1e-6, "{wake:?}");
    }

    #[test]
    fn a_head_that_moves_quickly_wakes_the_bar_at_its_own_pace() {
        let wake = next_wakeup(10.0, 30.0, 0.02);
        assert!((wake.as_secs_f64() - 0.021).abs() < 1e-6, "{wake:?}");
    }

    #[test]
    fn wakeups_stay_between_the_floor_and_the_ceiling() {
        assert_eq!(next_wakeup(10.0, 240.0, 0.0).as_secs_f64(), SHORTEST_WAKEUP);
        assert_eq!(next_wakeup(10.0, 240.0, 30.0).as_secs_f64(), LONGEST_WAKEUP);
    }

    #[test]
    fn rounding_boundaries_are_found_in_both_directions() {
        assert!((until_round_rises(3.2) - 0.3).abs() < 1e-9);
        assert!((until_round_rises(3.5) - 1.0).abs() < 1e-9);
        assert!((until_round_rises(3.9) - 0.6).abs() < 1e-9);
        assert!((until_round_falls(3.7) - 0.2).abs() < 1e-6);
        assert!((until_round_falls(3.2) - 0.7).abs() < 1e-6);
        assert!((until_round_falls(3.5) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn the_playhead_never_passes_the_end_or_zero() {
        assert_eq!(extrapolate((199.9, 0), 250_000, true, 200.0), 200.0);
        assert_eq!(extrapolate((-3.0, 0), 0, false, 200.0), 0.0);
        assert_eq!(extrapolate((5.0, 10), 0, true, 0.0), 5.0);
    }
}
