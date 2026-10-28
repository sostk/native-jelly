//! The strings sent to the real HUD draw, measured with the shipped font advances.
use super::*;
use nj_machine::machine::Measure;
use crate::ui::draw_census::{capture_runs, TextRun};
use crate::ui::marquee::Clock;

const FM: crate::ui::fixture::FixtureMeasure = crate::ui::fixture::FixtureMeasure;

fn runs(kicker: Kicker, title: &std::ffi::CStr, right: f32, measure: &dyn Measure, paused: bool, clock: &Clock) -> Vec<TextRun> {
    capture_runs(|| draw_player_title(Painter::recording(), kicker, title.as_ptr(), right, measure, paused, clock))
}

#[test]
fn long_hud_titles_fit_before_the_control_row() {
    let measure = &nj_base::fontcov::advances::ShippedMeasure;
    let long = CString::new("An extraordinarily long episode — Über den Wolken — ".repeat(20)).unwrap();
    for language in nj_platform::i18n::SHIPPED {
        let _lang = nj_platform::i18n::language_on_this_thread_for_test(language);
        let marker = |kind, final_seg| crate::metadata::Marker { kind, start_ms: 1000, end_ms: 2000, final_seg };
        let slots = [ControlSlot::Discs(crate::appkit::player_hud::Controls::NONE),
            ControlSlot::Discs(Controls::for_item(true, false, true, false)),
            slot_for(Some(marker(crate::metadata::MarkerKind::Intro, false)), false, crate::route::NextEpisodeMode::Countdown),
            slot_for(Some(marker(crate::metadata::MarkerKind::Credits, true)), true, crate::route::NextEpisodeMode::Countdown)];
        for slot in slots {
            let mut row = TransportRow::new();
            for focused in 0..slot.items() {
                for _ in 0..30 {
                    row.pop.step(Some(focused as usize), 1.0 / 60.0);
                    let right = title_right(slot, &row, measure);
                    for i in 0..slot.items() {
                        let r = slot.item_rect(&row, i, measure).unwrap();
                        assert!(right + theme::space::MD <= r.scaled(row.scale(i)).x + 0.001);
                    }
                    for kicker in [Kicker::Episode(long.as_ptr()), Kicker::Context(long.as_ptr())] {
                        let drawn = runs(kicker, &long, right, measure, false, &row.title_clock);
                        assert_eq!(drawn.len(), 2);
                        for run in drawn {
                            assert!(run.text.ends_with('…'), "{}: {run:?}", language.tag());
                            assert_eq!(run.at.x, SB_X);
                            assert!(measure.width_str(&run.text, run.size, run.bold) <= right - SB_X + 1.0,
                                "{}: HUD text overlaps the control row: {run:?}", language.tag());
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn short_hud_titles_keep_their_position_typography_and_ink() {
    for kicker in [Kicker::Episode(c"S1 · E1 · Pilot".as_ptr()), Kicker::Context(c"Trailer".as_ptr())] {
        let original = capture_runs(|| draw_title(Painter::recording(), kicker, c"Short title".as_ptr()));
        for paused in [false, true] {
            let row = TransportRow::new();
            let bounded = runs(kicker, c"Short title", title_right(ControlSlot::Discs(crate::appkit::player_hud::Controls::NONE), &row, &FM), &FM, paused, &row.title_clock);
            assert_eq!(original, bounded);
        }
    }
}

#[test]
fn paused_title_reveals_once_and_resume_restores_ellipsis() {
    let _serial = nj_base::testlock::serial();
    let row = TransportRow::new();
    let long = c"A very long movie title that should slowly reveal its final words after pausing";
    let right = title_right(ControlSlot::Discs(crate::appkit::player_hud::Controls::NONE), &row, &FM);
    let paint = |paused| runs(Kicker::Context(c"Movie".as_ptr()), long, right, &FM, paused, &row.title_clock);
    assert!(paint(true)[1].text.ends_with('…'));
    nj_machine::idle::frame_begin(2.0);
    let moving = paint(true);
    assert!(moving[1].at.x < SB_X);
    nj_machine::idle::frame_begin(60.0);
    let end = paint(true);
    assert_eq!(end[1].at.x, SB_X - (FM.width(long, HUD_TITLE_SZ, true) - (right - SB_X)));
    assert_eq!(paint(true), end, "the full ending stays visible");
    assert!(paint(false)[1].text.ends_with('…'));
    assert!(paint(true)[1].text.ends_with('…'), "a later pause restarts the reveal");
}

#[test]
fn hidden_titles_restart_and_null_titles_release_the_clock() {
    let _serial = nj_base::testlock::serial();
    let row = TransportRow::new();
    let long = c"A very long movie title that should slowly reveal its final words after pausing";
    let right = title_right(ControlSlot::Discs(crate::appkit::player_hud::Controls::NONE), &row, &FM);
    let paint = || runs(Kicker::Context(c"Movie".as_ptr()), long, right, &FM, true, &row.title_clock);
    paint();
    nj_machine::idle::frame_begin(2.0);
    assert!(paint()[1].at.x < SB_X);
    row.reset_title();
    assert!(paint()[1].text.ends_with('…'));
    nj_machine::idle::frame_begin(2.0);
    capture_runs(|| draw_player_title(Painter::recording(), Kicker::Context(c"Movie".as_ptr()),
        std::ptr::null(), right, &FM, true, &row.title_clock));
    assert!(paint()[1].text.ends_with('…'));
}
