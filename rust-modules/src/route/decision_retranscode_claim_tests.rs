//! A claimed user `Retranscode` (a live track or subtitle pick on a converted stream) runs its
//! negotiation on a worker, never the frame thread. These grade the worker's landing, its failure
//! paths and its staleness checks against a loopback Jellyfin.

use super::*;
use super::test_support::*;
use super::test_support::apply_plan;

fn transcoding_info() -> String {
    let url = format!(
        "/videos/{JF_GUID}/master.m3u8?VideoCodec=h264&AudioCodec=aac&TranscodeReasons=VideoCodecNotSupported&PlaySessionId=ps-loopback"
    );
    playback_info(
        false,
        "mkv",
        r#"{"Type":"Video","Codec":"hevc","Index":0},{"Type":"Audio","Codec":"eac3","Index":1,"Channels":6}"#,
        Some(&url),
    )
}

fn loopback(info: String, delay: std::time::Duration) -> JfLoopback {
    assert!(nj_net::net::global_init() && crate::curlio::available());
    JfLoopback::start_slow(info, user_config(None, true, None, "Default"), delay)
}

/// Land a converted stream of the loopback item, as a cold resolve would have.
fn install(ps: &mut PlaybackSession, lb: &JfLoopback) {
    let rk = jf_rk();
    apply_plan(
        ps,
        Plan {
            sid: lb.sid,
            url: format!("http://127.0.0.1/videos/{JF_GUID}/master.m3u8?PlaySessionId=first"),
            tsession: "claim-encoder-1".into(),
            sess: "claim-logical".into(),
            vcodec: "h264".into(),
            acodec: "aac".into(),
            src_vcodec: "hevc".into(),
            src_acodec: "eac3".into(),
            transport_kbps: 28_000,
            audio: Some(CarriedAudio {
                sid: 2,
                ordinal: 0,
                codec: "eac3".into(),
                channels: 6,
                can_normalize_loudness: false,
                immersive: false,
            }),
            ..Default::default()
        },
        &rk,
    );
}

fn queue_retranscode(ps: &PlaybackSession) -> ClaimedRouteAction {
    request_user_route_intent(ps, UserRouteIntent::Retranscode);
    claim_route_action().expect("a queued user action")
}

fn wait_until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !ready() {
        assert!(std::time::Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

/// Drain a claim worker's landing the way the pump does on a later frame.
fn await_landing(ps: &mut PlaybackSession, what: &str) -> (ClaimedRouteAction, ClaimTail) {
    let mut landed = None;
    wait_until(what, || {
        landed = take_ready_retranscode_claim(ps);
        landed.is_some()
    });
    let (action, tail, ..) = landed.expect("wait_until returns only once a landing was drained");
    (action, tail)
}

fn dispatch_tail(ps: &mut PlaybackSession, action: &ClaimedRouteAction) -> ClaimTail {
    match execute_retranscode_claim(ps, action, 60, -1, 0) {
        RetranscodeClaimDispatch::Sync(tail) => tail,
        RetranscodeClaimDispatch::Pending => await_landing(ps, "retranscode claim worker never landed").1,
    }
}

/// What the pump's tail does with a claim's result.
fn settle(ps: &mut PlaybackSession, action: &ClaimedRouteAction, tail: ClaimTail) {
    let result = match tail {
        ClaimTail::Rejected(_) => RouteApplyResult::Rejected,
        _ => RouteApplyResult::Prepared,
    };
    finish_route_action(ps, action, result);
}

fn phase() -> ControlPhase {
    PLAYER_CONTROL.lock().unwrap_or_else(|e| e.into_inner()).phase
}

fn cleanup(ps: &mut PlaybackSession) {
    let _ = take_pending_original();
    restore_quality(Quality::Original);
    reset_session(ps);
    install_active_encoder("");
    reset_player_control_for_test(ps);
    crate::player::reset_audio_track();
    crate::player::reset_subtitle();
}

#[test]
#[cfg(feature = "devtriggers")]
fn a_claimed_retranscode_negotiates_on_a_worker_and_lands_the_new_stream() {
    let mut ps = PlaybackSession::IDLE;
    let _g = fresh_registry(&mut ps);
    let lb = loopback(transcoding_info(), std::time::Duration::ZERO);
    install(&mut ps, &lb);
    let action = queue_retranscode(&ps);

    let frame = nj_base::task::FrameScope::enter();
    let dispatch = execute_retranscode_claim(&mut ps, &action, 60, -1, 0);
    drop(frame);
    assert!(
        matches!(dispatch, RetranscodeClaimDispatch::Pending),
        "the negotiation must leave the frame thread, got {dispatch:?}",
    );
    let (action, tail) = await_landing(&mut ps, "retranscode claim worker never landed");
    assert_eq!(tail, ClaimTail::Retranscode);
    settle(&mut ps, &action, tail);
    assert!(ps.url.contains("PlaySessionId=ps-loopback"), "{}", ps.url);
    assert_ne!(ps.tsession, "claim-encoder-1", "the rebuild runs under a fresh encoder session");
    assert_eq!((ps.stream_vcodec.as_str(), ps.stream_acodec.as_str()), ("h264", "aac"));
    assert_eq!(phase(), ControlPhase::Stable);

    let requests = lb.finish();
    let body = playback_info_body(&requests);
    assert_eq!(body["AudioStreamIndex"], 1, "the carried track, by stream index");
    assert_eq!(body["StartTimeTicks"], 60 * 10_000_000_i64, "the claim offset");
    cleanup(&mut ps);
}

/// A refused spawn (the OS could not create the worker thread) settles like a refused
/// negotiation: the reducer returns to `Stable` and the session is untouched.
#[test]
#[cfg(feature = "devtriggers")]
fn a_refused_spawn_settles_like_a_refusal_not_a_stuck_applying() {
    let mut ps = PlaybackSession::IDLE;
    let _g = fresh_registry(&mut ps);
    let lb = loopback(transcoding_info(), std::time::Duration::ZERO);
    install(&mut ps, &lb);
    let action = queue_retranscode(&ps);

    inject_next_fault(Fault::SpawnRefusal);
    let tail = match execute_retranscode_claim(&mut ps, &action, 60, -1, 0) {
        RetranscodeClaimDispatch::Sync(tail) => tail,
        RetranscodeClaimDispatch::Pending => panic!("a forced spawn refusal must resolve synchronously"),
    };
    assert!(matches!(tail, ClaimTail::Rejected(_)), "got {tail:?}");
    settle(&mut ps, &action, tail);
    assert_eq!(phase(), ControlPhase::Stable, "a refused spawn must not leave the reducer Applying");
    assert_eq!(ps.tsession, "claim-encoder-1", "nothing reached the server; the session is untouched");

    let action = queue_retranscode(&ps);
    let tail = dispatch_tail(&mut ps, &action);
    assert_eq!(tail, ClaimTail::Retranscode, "the reducer is usable again");
    settle(&mut ps, &action, tail);
    lb.finish();
    cleanup(&mut ps);
}

/// A worker that panics mid-negotiation still posts a rejection, so `Applying` is released
/// instead of waiting forever for a landing that will never arrive.
#[test]
#[cfg(feature = "devtriggers")]
fn a_worker_panic_still_releases_applying() {
    let mut ps = PlaybackSession::IDLE;
    let _g = fresh_registry(&mut ps);
    let lb = loopback(transcoding_info(), std::time::Duration::ZERO);
    install(&mut ps, &lb);
    let action = queue_retranscode(&ps);

    inject_next_fault(Fault::WorkerPanic);
    let dispatch = execute_retranscode_claim(&mut ps, &action, 60, -1, 0);
    assert!(matches!(dispatch, RetranscodeClaimDispatch::Pending));
    let (action, tail) = await_landing(&mut ps, "a panicked worker left Applying stuck");
    assert_eq!(tail, ClaimTail::Rejected(RETRANSCODE_WORKER_PANICKED));
    settle(&mut ps, &action, tail);
    assert_eq!(phase(), ControlPhase::Stable, "the panic must still release the reducer");
    assert_eq!(ps.tsession, "claim-encoder-1");

    let action = queue_retranscode(&ps);
    let tail = dispatch_tail(&mut ps, &action);
    assert_eq!(tail, ClaimTail::Retranscode, "the reducer is usable again after the panic");
    settle(&mut ps, &action, tail);
    lb.finish();
    cleanup(&mut ps);
}

/// A route event that lands while the worker waits on the server (a teardown here) invalidates
/// the ticket the worker snapshotted: its landing is discarded without touching the session, the
/// session it started is stopped, and a fresh claim applies normally.
#[test]
#[cfg(feature = "devtriggers")]
fn a_ticket_invalidated_mid_flight_is_discarded_then_a_fresh_claim_applies() {
    let mut ps = PlaybackSession::IDLE;
    let _g = fresh_registry(&mut ps);
    let lb = loopback(transcoding_info(), std::time::Duration::from_millis(150));
    install(&mut ps, &lb);
    let action = queue_retranscode(&ps);
    let dispatch = execute_retranscode_claim(&mut ps, &action, 60, -1, 0);
    assert!(matches!(dispatch, RetranscodeClaimDispatch::Pending), "got {dispatch:?}");

    begin_engine_teardown(true);

    let (action, tail) = await_landing(&mut ps, "retranscode claim worker never landed");
    assert!(matches!(tail, ClaimTail::Rejected(_)), "a stale worker must be discarded, got {tail:?}");
    settle(&mut ps, &action, tail);
    assert_eq!(ps.tsession, "claim-encoder-1", "the discarded worker never touched the session");
    assert_eq!(phase(), ControlPhase::Stable, "the discard must release the reducer");

    let action = queue_retranscode(&ps);
    let tail = dispatch_tail(&mut ps, &action);
    assert_eq!(tail, ClaimTail::Retranscode, "the current pick still applies");
    settle(&mut ps, &action, tail);
    lb.finish();
    cleanup(&mut ps);
}

/// The server refusing the rebuild keeps the current stream: a rejection, the encoder session
/// and URL unchanged.
#[test]
#[cfg(feature = "devtriggers")]
fn a_refused_rebuild_keeps_the_current_stream() {
    let mut ps = PlaybackSession::IDLE;
    let _g = fresh_registry(&mut ps);
    let lb = loopback(r#"{"MediaSources":[],"ErrorCode":"NoCompatibleStream"}"#.into(), std::time::Duration::ZERO);
    install(&mut ps, &lb);
    let url = ps.url.clone();
    let action = queue_retranscode(&ps);
    let tail = dispatch_tail(&mut ps, &action);
    assert_eq!(tail, ClaimTail::Rejected(RETRANSCODE_REJECTED));
    settle(&mut ps, &action, tail);
    assert_eq!((ps.tsession.as_str(), ps.url.as_str()), ("claim-encoder-1", url.as_str()));
    assert_eq!(phase(), ControlPhase::Stable);
    lb.finish();
    cleanup(&mut ps);
}
