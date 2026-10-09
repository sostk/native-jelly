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

/// Nanoseconds in one tick.
const NS_PER_TICK: i64 = 100;

/// Ticks → nanoseconds (the player clock), saturating; negative input is 0.
pub fn to_ns(ticks: i64) -> i64 {
    ticks.max(0).saturating_mul(NS_PER_TICK)
}

/// Ticks from .NET's epoch (0001-01-01T00:00:00Z) to the Unix one (1970-01-01T00:00:00Z).
///
/// `PlaybackStartTimeTicks` is a `DateTime.UtcNow.Ticks`, which counts from the year 1 — not from
/// 1970. Sending a bare Unix timestamp in ticks dates every session to the second century and the
/// server's session duration comes out two millennia wrong.
const UNIX_EPOCH_TICKS: i64 = 621_355_968_000_000_000;

/// Now, as the `DateTime.UtcNow.Ticks` the session reports carry. Before 1970 (an unset clock) it
/// clamps to the Unix epoch rather than reporting a negative instant the server cannot parse.
pub fn now_utc() -> i64 {
    let since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() / 100)
        .unwrap_or(0);
    UNIX_EPOCH_TICKS.saturating_add(i64::try_from(since_epoch).unwrap_or(i64::MAX - UNIX_EPOCH_TICKS))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_measured_runtime_is_eight_thousand_seconds_not_eight_hundred() {
        assert_eq!(to_secs(81_772_160_000), 8177);
        assert_eq!(to_ms(81_772_160_000), 8_177_216);
        assert_eq!(to_ns(81_772_160_000), 8_177_216_000_000);
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
        assert_eq!(to_ns(-1), 0);
        assert_eq!(to_ns(i64::MAX), i64::MAX);
    }

    #[test]
    fn utc_now_is_counted_from_the_dotnet_epoch_not_the_unix_one() {
        // 2020-01-01T00:00:00Z is 637_134_336_000_000_000 .NET ticks; anything near the Unix
        // epoch's own tick count would be a date in the year 1970 BC as far as the server is
        // concerned.
        let now = now_utc();
        assert!(now > 637_134_336_000_000_000, "ticks {now} predate 2020");
        assert_eq!(UNIX_EPOCH_TICKS / TICKS_PER_SECOND, 62_135_596_800, "1970 is 62135596800s after year 1");
    }
}
