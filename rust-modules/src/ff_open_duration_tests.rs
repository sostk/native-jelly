//! The total a progressive open publishes (`open_duration_ns`).
//!
//! Regression: a transcoded playback showed no total length. Jellyfin's default conversion is a
//! live progressive Matroska over http (`Content-Length: -1`); ffmpeg writes it to a pipe and so
//! never fills in Segment/Duration, `fmt_duration` reads 0 (or `AV_NOPTS_VALUE`), and
//! `SHARED.duration_ns` stayed 0 for the whole playback — no HUD total, no `/Sessions/Playing`
//! or `/Progress`, no `Stopped` (so no resume point), never marked watched, and no end cap on a
//! scrub. Direct play read the file header's duration; the HLS conversion publishes the
//! playlist total.

use super::*;

/// The measured 2 h 16 min film (`jf/ticks.rs`): `RunTimeTicks = 81_772_160_000`.
const FILM_NS: i64 = 8_177_216_000_000;

#[test]
fn a_live_transcode_with_no_container_duration_publishes_the_items_runtime() {
    assert_eq!(open_duration_ns(0, FILM_NS), Some(FILM_NS), "Matroska written to a pipe: duration 0");
    assert_eq!(open_duration_ns(AV_NOPTS_VALUE, FILM_NS), Some(FILM_NS), "no duration at all");
    assert_eq!(open_duration_ns(-5, FILM_NS), Some(FILM_NS));
}

#[test]
fn a_real_container_duration_still_wins() {
    // A direct-played file's header, in µs: what the open always published, and still does —
    // even when the server's runtime disagrees with it.
    assert_eq!(open_duration_ns(8_177_000_000, FILM_NS), Some(8_177_000_000_000));
    assert_eq!(open_duration_ns(8_177_000_000, 0), Some(8_177_000_000_000));
}

#[test]
fn nothing_known_publishes_nothing() {
    // Leaves what is there: a reload keeps the total across `reset_session_for_reload`, and a
    // fresh session's 0 stays the honest "unknown".
    assert_eq!(open_duration_ns(0, 0), None);
    assert_eq!(open_duration_ns(AV_NOPTS_VALUE, -1), None);
}

/// A transcode resumed or sought restarts the encode at `&offset`, and the display base carries
/// that offset, so the playbar is in CONTENT time. The total must be the whole film's, not the
/// remainder the encode will produce — and it must survive the reload's session reset.
#[test]
fn a_resumed_transcode_publishes_the_whole_film_and_a_reload_keeps_it() {
    let _g = nj_base::testlock::serial();
    let resume_ns = 4_000_000_000_000;
    SHARED.reset_session();
    SHARED.disp_base.store(resume_ns, Ordering::Relaxed);

    let published = publish_open_duration(0, FILM_NS);
    let kept_after_open = SHARED.duration_ns.load(Ordering::Relaxed);
    // A seek's reload: the Engine goes, the playback stays.
    SHARED.reset_session_for_reload();
    let kept_after_reload = SHARED.duration_ns.load(Ordering::Relaxed);
    // ...and the reopened encoder (offset moved again) publishes the same whole-film total.
    SHARED.disp_base.store(6_000_000_000_000, Ordering::Relaxed);
    let republished = publish_open_duration(0, FILM_NS);
    SHARED.reset_session();
    SHARED.disp_base.store(0, Ordering::Relaxed);

    assert_eq!(published, Some(FILM_NS));
    assert_eq!(kept_after_open, FILM_NS, "the whole film, not FILM - resume");
    assert_eq!(kept_after_reload, FILM_NS, "a reload keeps the total");
    assert_eq!(republished, Some(FILM_NS));
}
