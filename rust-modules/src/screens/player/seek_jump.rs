//! Dedicated FF/REW taps, distinct from ordinary D-pad preview gestures.
use super::*;

pub(super) fn seek_stays_hidden(ps: &crate::route::PlaybackSession, dismissed: bool) -> bool {
    use crate::player::{self, PlaybackState};
    if !dismissed { return false; }
    let state = player::state(ps);
    state == PlaybackState::Seeking || (state == PlaybackState::Buffering
        && !player::SHARED.hls_rebuffering.load(std::sync::atomic::Ordering::Relaxed)
        && player::SHARED.seek_display_ns.load(std::sync::atomic::Ordering::Relaxed) >= 0)
}

impl PlayerScreen {
    /// The newest tap remains the viewer's position until it can safely reach the pipeline.
    pub(super) fn jump_position(&self, ps: &crate::route::PlaybackSession) -> i64 {
        self.queued_jump.unwrap_or_else(|| crate::player::intended_pos_ns(ps))
    }

    /// Background suspension saves this target into its reload, consuming the deferred request.
    pub(crate) fn take_queued_jump(&mut self) -> Option<i64> { self.queued_jump.take() }

    pub(super) fn step_queued_jump<H: AppLike>(&mut self, ps: &crate::route::PlaybackSession,
        fx: &mut Effects<'_, H>) {
        use crate::player::{self, PlaybackState};
        match player::state(ps) {
            PlaybackState::Error => { self.queued_jump = None; }
            PlaybackState::Playing if !player::seek_preroll_active() => {
                if let Some(target) = self.queued_jump.take() {
                    Self::ask(fx, PlayerReq::CommitSeek(target));
                }
            }
            _ => {}
        }
    }

    pub(super) fn drop_transport_tap(&mut self) {
        if self.transport_tap {
            self.transport_tap = false;
            self.scrub.ns = -1;
            self.scrub.disengage();
            self.scrub.commit_at = 0;
        }
    }

    fn commit_jump<H: AppLike>(&mut self, ps: &crate::route::PlaybackSession,
        forward: bool, now: u32, fx: &mut Effects<'_, H>) {
        let duration = crate::player::duration_ns();
        if duration <= 0 { return; }
        let step = input::scrub_step_ns() * if forward { 1 } else { -1 };
        let position = self.jump_position(ps);
        let target = self.seek_burst.jump(now, position, step, duration);
        self.scrub.ns = -1;
        self.scrub.disengage();
        self.scrub.commit_at = 0;
        self.hud.until = 0;
        self.hud.dismissed = true;
        // A second av_seek while the first keyframe is still queued can anchor stale packets.
        // Keep only the latest destination until both the seek and paused-frame prime finish.
        if crate::player::loading(ps) || crate::player::seek_preroll_active() {
            self.queued_jump = Some(target);
        } else {
            self.queued_jump = None;
            Self::ask(fx, PlayerReq::CommitSeek(target));
        }
        self.publish();
    }

    pub(super) fn jump_key<H: AppLike>(&mut self, ps: &crate::route::PlaybackSession,
        key: consts::Key, edge: Edge, now: u32, fx: &mut Effects<'_, H>) -> bool {
        let (forward, transport) = match key {
            consts::Key::Left { alt } => (false, alt),
            consts::Key::Right { alt } => (true, alt),
            _ => return false,
        };
        if !transport {
            if edge == Edge::Down { self.drop_transport_tap(); }
            if crate::route::instant_dpad_seeking() == crate::route::InstantDpadSeeking::On
                && !self.hud.visible_at_press {
                if edge == Edge::Down { self.commit_jump(ps, forward, now, fx); }
                return true;
            }
            self.seek_burst.reset();
            return false;
        }
        if crate::player::duration_ns() <= 0 {
            self.drop_transport_tap();
            return true;
        }
        match edge {
            Edge::Down => {
                self.transport_tap = true;
                self.scrub.begin(now, forward);
                self.scrub.ns = self.jump_position(ps);
                self.scrub.reveal = true;
                self.hud.nav.focus = 0;
                if !self.hud.visible_at_press { self.hud.until = 0; self.hud.dismissed = true; }
            }
            Edge::Repeat => {
                self.transport_tap = false;
                self.queued_jump = None;
                self.seek_burst.reset();
                self.hud.dismissed = false;
                self.key_scrub_repeat(now);
                self.hud.extend(now, input::HUD_LINGER_MS);
            }
            Edge::Up => {
                if self.transport_tap {
                    self.transport_tap = false;
                    self.commit_jump(ps, forward, now, fx);
                } else { self.key_scrub_release(now, fx); }
            }
        }
        self.publish();
        true
    }
}
