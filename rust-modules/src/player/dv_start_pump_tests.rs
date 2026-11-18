//! The Dolby Vision landing guard driven through the real `pump()` (upstream PlxNative #1166): a
//! declared landing the firmware's splitter would skip (the row), or that no position update follows
//! (the watch), is one `reload_at` without the node. A sibling of `native_seek_pump_tests` and built
//! on its `Rig`; every case builds the guard through `engine::dv_start_for`, the only constructor
//! production uses, and plants the firmware row where production reaches it: on the ATTEMPT's caps.
//! `reload_at` has no route in a unit rig, so each call is read off `engine::RELOAD_AT_LOG`.

use super::native_seek_pump_tests::{reloads, Rig, TARGET_NS};
use super::*;
use crate::metadata::Dovi;
use crate::player::dv_start::Mode;
use crate::player::ffi_host;
use nj_platform::devcaps::{apply_dv_first_picture, dv::DvCapability, Caps};
use std::os::raw::c_int;

/// An HEVC CRA as an access unit's `key` (`aq::AuNode::key`).
const CRA: c_int = 21;
const IDR: c_int = crate::aq::AU_KEY;
/// A tap taken while a claim is out, later than the seek that was armed.
const TAP_NS: i64 = 1_200_000_000_000;

fn p81() -> Dovi {
    Dovi { present: true, profile: 8, bl_compat: 1, el_present: false, ..Dovi::NONE }
}

fn p5() -> Dovi {
    Dovi { present: true, profile: 5, bl_compat: 0, el_present: false, ..Dovi::NONE }
}

/// The caps of the C1 the row is for (`kisscurl-koli`), applied as the boot probe does.
fn row_caps() -> std::sync::Arc<Caps> {
    let mut c = Caps::assumed();
    assert!(apply_dv_first_picture(&mut c, "kisscurl-koli"));
    std::sync::Arc::new(c)
}

/// Build the Engine's guard the way `start_bufferfeed` does, from the rig's route.
fn arm_guard(rig: &mut Rig, armed: Option<i64>) {
    let guard = super::super::engine::dv_start_for(&rig.ps, armed);
    rig.eng().dv_start = guard;
}

/// A declared direct play of `dovi`, on a set with the row when `row`.
fn declared(dovi: Dovi, row: bool) -> Rig {
    let mut rig = Rig::new(None);
    assert!(crate::route::set_stream_declaration_for_test(
        &mut rig.ps,
        "hevc",
        "eac3",
        23.976,
        dovi,
        false,
        DvCapability::Supported,
    ));
    let caps = if row { row_caps() } else { std::sync::Arc::new(Caps::assumed()) };
    crate::route::set_attempt_caps_for_test(&mut rig.ps, caps);
    arm_guard(&mut rig, None);
    rig
}

fn is_declared(rig: &Rig) -> bool {
    crate::route::stream_dv_presentation(&rig.ps).declared().is_some()
}

fn frames(rig: &mut Rig, n: usize) {
    for _ in 0..n {
        rig.frame();
    }
}

/// Advance the pump clock by `ms`, a frame per 250 ms.
fn advance(rig: &mut Rig, ms: u32) {
    for _ in 0..ms / 250 {
        rig.now += 250;
        rig.frame();
    }
}

/// An in-place seek to `TARGET_NS` whose landing keyframe carries `key`, then enough frames for the
/// pump to meet it, feed what follows and release the prime.
fn seek_and_land(rig: &mut Rig, key: c_int) {
    rig.arm(TARGET_NS);
    rig.land_keyframe_with_key(TARGET_NS, 90, key);
    frames(rig, 12);
}

#[test]
fn a_native_seek_landing_on_a_cra_under_the_c1_row_reloads_once_without_dolby_vision() {
    let _serial = nj_base::testlock::serial();
    let mut rig = declared(p81(), true);
    seek_and_land(&mut rig, CRA);
    assert!(ffi_host::seek_log_for_test().is_empty(), "the keyframe the splitter would skip is not fed, so no native Seek");
    assert_eq!(reloads(), vec![TARGET_NS], "one reload at the seek's target");
    assert_eq!(crate::route::stream_dv_presentation(&rig.ps), crate::metadata::DvPresentation::NotDv);
    assert_eq!(rig.eng().dv_start.mode(), Mode::Act);
}

#[test]
fn the_same_landing_on_an_idr_seeks_in_place_as_before() {
    let _serial = nj_base::testlock::serial();
    let mut rig = declared(p81(), true);
    seek_and_land(&mut rig, IDR);
    assert_eq!(ffi_host::seek_log_for_test(), vec![TARGET_NS / 1_000_000], "one native Seek, at the keyframe");
    assert!(reloads().is_empty());
    assert!(is_declared(&rig), "an IDR is what the splitter takes: Dolby Vision stays");
}

#[test]
fn with_no_row_a_declared_landing_with_no_position_update_reloads_without_dolby_vision_after_4_s() {
    let _serial = nj_base::testlock::serial();
    let mut rig = declared(p81(), false);
    seek_and_land(&mut rig, CRA);
    assert!(reloads().is_empty(), "no row, no prediction: the keyframe is fed");
    advance(&mut rig, 3_000);
    assert!(reloads().is_empty(), "inside the 4 s");
    advance(&mut rig, 7_000);
    assert_eq!(reloads(), vec![TARGET_NS], "one reload, after 10 s of frames");
    assert!(!is_declared(&rig));
}

#[test]
fn a_position_update_before_4_s_keeps_dolby_vision() {
    let _serial = nj_base::testlock::serial();
    let mut rig = declared(p81(), false);
    seek_and_land(&mut rig, CRA);
    advance(&mut rig, 2_000);
    SHARED.frames.store(1, Relaxed);
    advance(&mut rig, 12_000);
    assert!(reloads().is_empty());
    assert!(is_declared(&rig));
}

#[test]
fn a_profile_5_landing_on_a_cra_is_fed_as_before() {
    let _serial = nj_base::testlock::serial();
    // Nothing can take over for a Profile 5 (no base layer): reported only.
    let mut rig = declared(p5(), true);
    assert_eq!(rig.eng().dv_start.mode(), Mode::ReportOnly);
    seek_and_land(&mut rig, CRA);
    assert_eq!(ffi_host::seek_log_for_test(), vec![TARGET_NS / 1_000_000], "fed as before");
    assert!(reloads().is_empty());
    assert!(is_declared(&rig));
}

#[test]
fn after_the_withdrawal_a_later_cra_seek_is_in_place() {
    let _serial = nj_base::testlock::serial();
    let mut rig = declared(p81(), true);
    assert!(crate::route::withdraw_dv_presentation(&mut rig.ps, "test"));
    // The reload's fresh Engine is built from the withdrawn route: no node, so nothing to guard.
    arm_guard(&mut rig, None);
    assert_eq!(rig.eng().dv_start.mode(), Mode::Inert);
    seek_and_land(&mut rig, CRA);
    assert_eq!(ffi_host::seek_log_for_test(), vec![TARGET_NS / 1_000_000], "in place, a native Seek");
    assert!(reloads().is_empty());
}

#[test]
fn a_withdrawal_deferred_by_a_claim_reloads_once_at_the_tap_taken_during_it() {
    let _serial = nj_base::testlock::serial();
    let mut rig = declared(p81(), true);
    rig.arm(TARGET_NS);
    // A claim is out when the row's landing arrives: the driver must wait for it.
    crate::route::force_applying_for_test(7);
    assert!(crate::route::claim_in_flight());
    rig.land_keyframe_with_key(TARGET_NS, 60, CRA);
    frames(&mut rig, 6);
    crate::player::request_seek(TAP_NS); // a tap during the claim
    frames(&mut rig, 4);
    assert!(reloads().is_empty(), "nothing reloads while the claim is out");
    rig.now += 5_000; // far past SEEK_STUCK_MS: the stuck watchdog must yield to the withdrawal
    frames(&mut rig, 4);
    assert!(reloads().is_empty(), "nor does the stuck watchdog reload with the node still declared");
    crate::route::reset_player_control_for_test(crate::route::idle_session_for_test());
    frames(&mut rig, 6);
    assert_eq!(reloads(), vec![TAP_NS], "exactly one reload, at the tap taken during the claim");
    assert!(!is_declared(&rig));
}
