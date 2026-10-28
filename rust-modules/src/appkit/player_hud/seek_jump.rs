//! Relative taps accumulate from the requested target, even after an earlier keyframe lands.
use super::scrub_clamp_target;
const BURST_GAP_MS: u32 = 1_500;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SeekBurst { last: Option<(i64, u32)> }
impl SeekBurst {
    pub(crate) const IDLE: Self = Self { last: None };
    pub(crate) fn reset(&mut self) { *self = Self::IDLE; }
    pub(crate) fn snapshot(&self) -> Option<(i64, u32)> { self.last }
    pub(crate) fn jump(&mut self, now: u32, position: i64, step: i64, duration: i64) -> i64 {
        let base = self.last.filter(|&(_, at)| now.wrapping_sub(at) <= BURST_GAP_MS)
            .map_or(position, |(target, _)| target);
        let target = scrub_clamp_target(base.saturating_add(step), duration);
        self.last = Some((target, now));
        target
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn burst_expires_resets_clamps_and_wraps_ticks() {
        let mut burst = SeekBurst::IDLE;
        let s = 1_000_000_000;
        assert_eq!(burst.jump(u32::MAX - 20, 40*s, 10*s, 100*s), 50*s);
        assert_eq!(burst.jump(20, 40*s, 10*s, 100*s), 60*s);
        assert_eq!(burst.jump(30, 40*s, -10*s, 100*s), 50*s);
        assert_eq!(burst.jump(1_531, 43*s, 10*s, 100*s), 53*s);
        assert_eq!(burst.jump(1_540, 0, 100*s, 100*s), 97*s);
        assert_eq!(burst.jump(1_550, 0, -100*s, 100*s), 0);
        burst.reset();
        assert_eq!(burst.jump(1_560, 43*s, 10*s, 100*s), 53*s);
    }
}
