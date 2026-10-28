//! Shared reading motion: a per-view clock and a one-shot horizontal reveal that rests at the end.
//! The HUD owns its clock, so hiding/reloading it cannot leak a reveal into another surface.
use std::cell::{Cell, RefCell};

/// The same couch-readable beat and speed used by focused tile titles.
pub(crate) const HOLD_MS: f32 = 1000.0;
pub(crate) const SPEED: f32 = 40.0;

#[derive(Default)]
pub(crate) struct Clock {
    key: RefCell<(String, String)>,
    start: Cell<Option<u32>>,
}

impl Clock {
    pub(crate) fn release(&self) {
        self.start.set(None);
    }

    /// Both title and context identify a run: a show's name can stay unchanged across episodes.
    pub(crate) fn read(&self, title: &str, context: &str) -> f32 {
        let now = nj_machine::idle::now_ms();
        let mut key = self.key.borrow_mut();
        if self.start.get().is_none() || key.0 != title || key.1 != context {
            key.0.clear();
            key.0.push_str(title);
            key.1.clear();
            key.1.push_str(context);
            self.start.set(Some(now));
        }
        now.wrapping_sub(self.start.get().unwrap_or(now)) as f32
    }
}

/// Reveal an overflowing line's end once. Fitting lines never schedule a frame; completed
/// reveals stop reporting, allowing the idle gate to close after the final frame is shown.
pub(crate) fn reveal_once(elapsed: f32, text_w: f32, budget: f32) -> f32 {
    let end = (text_w - budget).max(0.0);
    let offset = ((elapsed - HOLD_MS).max(0.0) / 1000.0 * SPEED).min(end);
    if offset < end {
        if elapsed < HOLD_MS {
            nj_machine::idle::wake();
        } else {
            nj_machine::idle::invalidate();
        }
    }
    offset
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_shot_reveal_reports_in_motion_and_rests_at_the_last_word() {
        let _serial = nj_base::testlock::serial();
        nj_machine::idle::reset_for_test();
        nj_machine::idle::note_present(10_000);
        nj_machine::idle::frame_begin(0.016);
        assert_eq!(reveal_once(0.0, 500.0, 200.0), 0.0);
        assert!(nj_machine::idle::should_present(10_016), "the hold must buy a future frame");
        nj_machine::idle::note_present(10_016);
        nj_machine::idle::frame_begin(0.016);
        assert_eq!(reveal_once(HOLD_MS + 2000.0, 500.0, 200.0), 80.0);
        assert!(nj_machine::idle::should_present(10_032));
        nj_machine::idle::note_present(10_032);
        nj_machine::idle::frame_begin(0.016);
        assert_eq!(reveal_once(1_000_000.0, 500.0, 200.0), 300.0);
        assert!(!nj_machine::idle::should_present(10_048), "the final words must rest");
        assert_eq!(reveal_once(1_000_000.0, 100.0, 200.0), 0.0);
        assert!(!nj_machine::idle::should_present(10_048), "a fitting title owes no frames");
    }

    #[test]
    fn clock_restarts_for_episode_changes_and_release() {
        let _serial = nj_base::testlock::serial();
        let clock = Clock::default();
        assert_eq!(clock.read("Show", "Episode 1"), 0.0);
        nj_machine::idle::frame_begin(2.0);
        assert_eq!(clock.read("Show", "Episode 1"), 2000.0);
        assert_eq!(clock.read("Show", "Episode 2"), 0.0);
        nj_machine::idle::frame_begin(2.0);
        assert_eq!(clock.read("Show", "Episode 2"), 2000.0);
        clock.release();
        assert_eq!(clock.read("Show", "Episode 2"), 0.0);
    }
}
