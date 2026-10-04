//! Jellyfin time is in TICKS — 100 ns units, `10_000_000` per second (.NET `TimeSpan`). The app is
//! in milliseconds (every `Metadata` offset, `TimelineReport::time_ms`) and nanoseconds (the player
//! clock) depending on the layer. **Every conversion between them is in this file**, so a factor of
//! ten thousand cannot be got wrong in one place and right in another.
//!
//! Measured on Jellyfin 12.0 (`docs/jf-spikes.md` S1): a 2 h 16 min film reports
//! `RunTimeTicks = 81_772_160_000`, i.e. 8177.216 s at 10^7 ticks per second.

/// Ticks in one millisecond.
pub const TICKS_PER_MS: i64 = 10_000;
/// Ticks in one second.
pub const TICKS_PER_SECOND: i64 = 10_000_000;

/// Ticks → milliseconds, truncating; negative input (a server sentinel) is 0.
pub fn to_ms(ticks: i64) -> i64 {
    ticks.max(0) / TICKS_PER_MS
}

/// Milliseconds → ticks, saturating; negative input is 0.
pub fn from_ms(ms: i64) -> i64 {
    ms.max(0).saturating_mul(TICKS_PER_MS)
}

/// Ticks → whole seconds, truncating.
pub fn to_secs(ticks: i64) -> i64 {
    ticks.max(0) / TICKS_PER_SECOND
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_measured_runtime_is_eight_thousand_seconds_not_eight_hundred() {
        assert_eq!(to_secs(81_772_160_000), 8177);
        assert_eq!(to_ms(81_772_160_000), 8_177_216);
    }

    #[test]
    fn a_progress_report_round_trips_to_the_millisecond() {
        for ms in [0, 1, 999, 43_578, 3_600_000, 8_177_216] {
            assert_eq!(to_ms(from_ms(ms)), ms);
        }
        assert_eq!(to_ms(435_781_990), 43_578);
    }

    #[test]
    fn negative_and_huge_values_neither_wrap_nor_panic() {
        assert_eq!(to_ms(-1), 0);
        assert_eq!(from_ms(-5), 0);
        assert_eq!(from_ms(i64::MAX), i64::MAX);
    }
}
