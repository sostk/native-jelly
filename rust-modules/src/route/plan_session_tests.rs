//! Playback-session classification tests: clip queue rows.

use super::*;
#[allow(unused_imports)]
use super::test_support::*;

#[test]
fn a_clip_queue_row_does_not_arm_up_next() {
    let clip = crate::catalog::QueueRow {
        kind: "clip".into(),
        rk: "9".into(),
        part: "/p".into(),
        ..Default::default()
    };
    assert!(up_next_of(&clip).is_none());
    let movie = crate::catalog::QueueRow {
        kind: "movie".into(),
        rk: "1".into(),
        ..Default::default()
    };
    assert!(up_next_of(&movie).is_none());
}
