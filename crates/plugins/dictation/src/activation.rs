//! How the dictation shortcut's presses and releases start and stop a
//! recording, in Handy's three activation modes. A port of Handy's
//! `CoordinatorState` (`src-tauri/src/transcription_coordinator.rs`, see
//! `docs/provenance/handy.md`) for one shortcut, without the release grace
//! that absorbs X11 key repeat: a plugin has no timer to end the grace.
//!
//! A recording starts on the press in every mode. What ends it differs:
//!
//! - push-to-talk: every release stops it
//! - toggle: releases do nothing, and the next press stops it
//! - hold or toggle: a release after a hold of at least
//!   [`HOLD_THRESHOLD_MS`] stops it, while a shorter tap keeps it
//!   recording until the next press
//!
//! A press while the last recording is still being transcribed is
//! remembered and starts a recording once that one is pasted, as Handy
//! does, so a toggle never loses track of which press stops.

/// How the shortcut starts and stops a recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Mode {
    /// Hold to record and release to stop, or tap to record until the
    /// next press. Handy's default.
    #[default]
    HoldOrToggle,
    /// Record while the shortcut is held.
    PushToTalk,
    /// Press to start, press again to stop.
    Toggle,
}

impl Mode {
    /// Each mode, its value in the settings file and its label.
    pub(crate) const ALL: [(Self, &'static str, &'static str); 3] = [
        (Self::HoldOrToggle, "hold-or-toggle", "Hold or tap"),
        (Self::PushToTalk, "push-to-talk", "Hold"),
        (Self::Toggle, "toggle", "Toggle"),
    ];

    pub(crate) fn value(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(mode, ..)| *mode == self)
            .map_or("hold-or-toggle", |(_, value, _)| value)
    }

    pub(crate) fn from_value(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .find(|(_, known, _)| *known == value)
            .map(|(mode, ..)| *mode)
    }

    /// The shortest hold, in milliseconds, whose release stops recording.
    fn hold_threshold(self) -> i64 {
        match self {
            Self::HoldOrToggle => HOLD_THRESHOLD_MS,
            Self::PushToTalk | Self::Toggle => 0,
        }
    }
}

/// Handy's default `hold_threshold_ms`: a shorter press is a tap.
pub(crate) const HOLD_THRESHOLD_MS: i64 = 300;

/// Presses closer together than this are one press, as from key repeat.
const DEBOUNCE_MS: i64 = 30;

/// What the plugin should do after a press or a release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Effect {
    Start,
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Idle,
    Recording,
    /// Stopped, and waiting for the text to paste.
    Processing,
}

/// The press that started the recording.
#[derive(Debug, Clone, Copy)]
struct Hold {
    pressed_at: i64,
    /// The recording outlives the key: the next press stops it.
    locked: bool,
}

/// A press that came while the last recording was being transcribed.
#[derive(Debug, Clone, Copy)]
struct Pending {
    pressed_at: i64,
    /// It starts a recording that outlives the key: a toggle press, or a
    /// tap.
    locked: bool,
}

/// The shortcut's state between events. Times are milliseconds on the
/// device's clock.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Activation {
    stage: Stage,
    hold: Option<Hold>,
    last_press: Option<i64>,
    pending: Option<Pending>,
}

impl Activation {
    pub(crate) const fn new() -> Self {
        Self {
            stage: Stage::Idle,
            hold: None,
            last_press: None,
            pending: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn is_recording(&self) -> bool {
        self.stage == Stage::Recording
    }

    /// Recording, or waiting for the text to paste.
    pub(crate) fn is_busy(&self) -> bool {
        self.stage != Stage::Idle
    }

    fn is_locked(&self) -> bool {
        self.hold.is_some_and(|hold| hold.locked) || self.pending.is_some_and(|press| press.locked)
    }

    /// The shortcut went down at `now`.
    pub(crate) fn press(&mut self, mode: Mode, now: i64) -> Option<Effect> {
        if self
            .last_press
            .is_some_and(|last| now.saturating_sub(last) < DEBOUNCE_MS)
        {
            return None;
        }
        self.last_press = Some(now);
        match self.stage {
            Stage::Processing => {
                match (mode, self.pending) {
                    // Toggle presses alternate, so two net to nothing.
                    (Mode::Toggle, Some(_)) => self.pending = None,
                    (_, None) => {
                        self.pending = Some(Pending {
                            pressed_at: now,
                            locked: mode == Mode::Toggle,
                        });
                    }
                    // A press on a remembered tap stops it.
                    (_, Some(Pending { locked: true, .. })) => self.pending = None,
                    // The key is already down.
                    (_, Some(Pending { locked: false, .. })) => {}
                }
                None
            }
            Stage::Idle => Some(self.begin_recording(now, mode == Mode::Toggle)),
            Stage::Recording => {
                // In toggle mode every press stops, even a recording that
                // began under a hold mode before you changed the setting.
                if self.is_locked() || mode == Mode::Toggle {
                    Some(self.begin_processing())
                } else {
                    // The key is still held: its release decides.
                    None
                }
            }
        }
    }

    /// The shortcut came up at `now`.
    pub(crate) fn release(&mut self, mode: Mode, now: i64) -> Option<Effect> {
        let hold_to_talk = mode != Mode::Toggle && !self.is_locked();
        if !hold_to_talk {
            return None;
        }
        let threshold = mode.hold_threshold();
        match self.stage {
            Stage::Recording => {
                let held = self
                    .hold
                    .map_or(i64::MAX, |hold| now.saturating_sub(hold.pressed_at));
                if held >= threshold {
                    return Some(self.begin_processing());
                }
                if let Some(hold) = &mut self.hold {
                    hold.locked = true;
                }
                None
            }
            Stage::Processing => {
                if let Some(pending) = &mut self.pending {
                    if now.saturating_sub(pending.pressed_at) < threshold {
                        // A tap while busy starts a locked recording later.
                        pending.locked = true;
                    } else {
                        // A whole hold while busy has nothing left to say.
                        self.pending = None;
                    }
                }
                None
            }
            Stage::Idle => None,
        }
    }

    /// The recording couldn't start, as when no model is downloaded.
    pub(crate) fn start_failed(&mut self) {
        if self.stage == Stage::Recording {
            self.stage = Stage::Idle;
            self.hold = None;
        }
    }

    /// The text is pasted, or the recording ended without any. A press
    /// remembered meanwhile starts the next recording.
    pub(crate) fn finished(&mut self) -> Option<Effect> {
        self.stage = Stage::Idle;
        self.hold = None;
        let pending = self.pending.take()?;
        Some(self.begin_recording(pending.pressed_at, pending.locked))
    }

    /// You cancelled. Nothing waits to start either.
    pub(crate) fn cancelled(&mut self) {
        self.stage = Stage::Idle;
        self.hold = None;
        self.pending = None;
    }

    fn begin_recording(&mut self, pressed_at: i64, locked: bool) -> Effect {
        self.stage = Stage::Recording;
        self.hold = Some(Hold { pressed_at, locked });
        Effect::Start
    }

    fn begin_processing(&mut self) -> Effect {
        self.stage = Stage::Processing;
        self.hold = None;
        Effect::Stop
    }
}

#[cfg(test)]
mod tests {
    use super::{Activation, Effect, HOLD_THRESHOLD_MS, Mode};

    #[test]
    fn hold_or_toggle_stops_on_the_release_of_a_long_hold() {
        let mut state = Activation::new();
        assert_eq!(state.press(Mode::HoldOrToggle, 0), Some(Effect::Start));
        assert_eq!(state.release(Mode::HoldOrToggle, 1_000), Some(Effect::Stop));
        assert!(state.is_busy());
        assert!(!state.is_recording());
    }

    #[test]
    fn hold_or_toggle_keeps_recording_after_a_tap_until_the_next_press() {
        let mut state = Activation::new();
        assert_eq!(state.press(Mode::HoldOrToggle, 0), Some(Effect::Start));
        assert_eq!(state.release(Mode::HoldOrToggle, 120), None);
        assert!(state.is_recording());
        // A locked recording ignores its release.
        assert_eq!(state.press(Mode::HoldOrToggle, 4_000), Some(Effect::Stop));
        assert_eq!(state.release(Mode::HoldOrToggle, 4_100), None);
    }

    #[test]
    fn the_hold_threshold_is_handys_300_ms() {
        let mut held = Activation::new();
        held.press(Mode::HoldOrToggle, 0);
        assert_eq!(
            held.release(Mode::HoldOrToggle, HOLD_THRESHOLD_MS),
            Some(Effect::Stop)
        );
        let mut tapped = Activation::new();
        tapped.press(Mode::HoldOrToggle, 0);
        assert_eq!(
            tapped.release(Mode::HoldOrToggle, HOLD_THRESHOLD_MS - 1),
            None
        );
        assert!(tapped.is_recording());
    }

    #[test]
    fn a_press_while_the_key_is_held_does_nothing() {
        let mut state = Activation::new();
        state.press(Mode::HoldOrToggle, 0);
        assert_eq!(state.press(Mode::HoldOrToggle, 500), None);
        assert_eq!(state.release(Mode::HoldOrToggle, 900), Some(Effect::Stop));
    }

    #[test]
    fn push_to_talk_stops_on_every_release_even_a_short_one() {
        let mut state = Activation::new();
        assert_eq!(state.press(Mode::PushToTalk, 0), Some(Effect::Start));
        assert_eq!(state.release(Mode::PushToTalk, 50), Some(Effect::Stop));
    }

    #[test]
    fn toggle_ignores_releases_and_stops_on_the_next_press() {
        let mut state = Activation::new();
        assert_eq!(state.press(Mode::Toggle, 0), Some(Effect::Start));
        assert_eq!(state.release(Mode::Toggle, 2_000), None);
        assert!(state.is_recording());
        assert_eq!(state.press(Mode::Toggle, 5_000), Some(Effect::Stop));
    }

    #[test]
    fn a_toggle_press_stops_a_recording_that_started_as_a_hold() {
        let mut state = Activation::new();
        state.press(Mode::HoldOrToggle, 0);
        assert_eq!(state.press(Mode::Toggle, 1_000), Some(Effect::Stop));
    }

    #[test]
    fn presses_inside_the_debounce_window_are_one_press() {
        let mut state = Activation::new();
        assert_eq!(state.press(Mode::Toggle, 0), Some(Effect::Start));
        assert_eq!(state.press(Mode::Toggle, 10), None);
        assert!(state.is_recording());
    }

    #[test]
    fn a_press_while_busy_starts_the_next_recording_once_the_text_is_pasted() {
        let mut state = Activation::new();
        state.press(Mode::Toggle, 0);
        state.press(Mode::Toggle, 1_000);
        assert_eq!(state.press(Mode::Toggle, 1_500), None);
        assert_eq!(state.finished(), Some(Effect::Start));
        assert!(state.is_recording());
        // It started locked, as a toggle press does.
        assert_eq!(state.release(Mode::Toggle, 1_600), None);
    }

    #[test]
    fn two_toggle_presses_while_busy_net_to_nothing() {
        let mut state = Activation::new();
        state.press(Mode::Toggle, 0);
        state.press(Mode::Toggle, 1_000);
        state.press(Mode::Toggle, 1_500);
        state.press(Mode::Toggle, 1_800);
        assert_eq!(state.finished(), None);
        assert!(!state.is_busy());
    }

    #[test]
    fn hold_or_toggle_a_tap_while_busy_queues_a_locked_recording() {
        let mut state = Activation::new();
        state.press(Mode::HoldOrToggle, 0);
        state.release(Mode::HoldOrToggle, 1_000);
        state.press(Mode::HoldOrToggle, 1_200);
        state.release(Mode::HoldOrToggle, 1_300);
        assert_eq!(state.finished(), Some(Effect::Start));
        // Locked: the next press stops it.
        assert_eq!(state.release(Mode::HoldOrToggle, 1_400), None);
        assert_eq!(state.press(Mode::HoldOrToggle, 3_000), Some(Effect::Stop));
    }

    #[test]
    fn hold_or_toggle_a_whole_hold_while_busy_nets_to_nothing() {
        let mut state = Activation::new();
        state.press(Mode::HoldOrToggle, 0);
        state.release(Mode::HoldOrToggle, 1_000);
        state.press(Mode::HoldOrToggle, 1_200);
        state.release(Mode::HoldOrToggle, 2_000);
        assert_eq!(state.finished(), None);
    }

    #[test]
    fn a_held_key_while_busy_starts_recording_unlocked_when_the_text_is_pasted() {
        let mut state = Activation::new();
        state.press(Mode::HoldOrToggle, 0);
        state.release(Mode::HoldOrToggle, 1_000);
        state.press(Mode::HoldOrToggle, 1_200);
        assert_eq!(state.finished(), Some(Effect::Start));
        // The hold counts from the real key-down.
        assert_eq!(state.release(Mode::HoldOrToggle, 1_600), Some(Effect::Stop));
    }

    #[test]
    fn a_failed_start_goes_back_to_idle() {
        let mut state = Activation::new();
        state.press(Mode::Toggle, 0);
        state.start_failed();
        assert!(!state.is_busy());
        assert_eq!(state.press(Mode::Toggle, 1_000), Some(Effect::Start));
    }

    #[test]
    fn cancelling_drops_a_remembered_press() {
        let mut state = Activation::new();
        state.press(Mode::Toggle, 0);
        state.press(Mode::Toggle, 1_000);
        state.press(Mode::Toggle, 1_500);
        state.cancelled();
        assert_eq!(state.finished(), None);
    }

    #[test]
    fn modes_read_back_from_their_values() {
        for (mode, value, _) in Mode::ALL {
            assert_eq!(Mode::from_value(value), Some(mode));
            assert_eq!(mode.value(), value);
        }
        assert_eq!(Mode::from_value("sometimes"), None);
        assert_eq!(Mode::default(), Mode::HoldOrToggle);
    }
}
