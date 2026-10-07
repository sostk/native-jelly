//! Strict original-stream override and ordinary disabled-direct-play routing.
use super::*;
use super::test_support::*;

#[test]
fn direct_play_modes_compose_with_each_quality_ceiling() {
    for q in QUALITY_LADDER {
        for measured in [(0, 0, 0), (3_000, 1280, 720), (60_000, 3840, 2160)] {
            let ordinary = quality_policy(q, false, measured.0, measured.1, measured.2);
            let auto = direct_play_policy(DirectPlayMode::Auto, ordinary);
            assert_eq!((auto.direct_play, auto.remux), (ordinary.direct_play, ordinary.remux));
            let disabled = direct_play_policy(DirectPlayMode::Disabled, ordinary);
            assert!(!disabled.direct_play);
            assert_eq!(disabled.remux, ordinary.remux);
            let forced = direct_play_policy(DirectPlayMode::Forced, ordinary);
            assert!(forced.direct_play && !forced.remux);
        }
    }
}

const HEVC_EAC3: &str = r#"{"Type":"Video","Codec":"hevc","Index":0},{"Type":"Audio","Codec":"eac3","Index":1,"Channels":6}"#;

/// One resolve of the loopback item under `configure`; the plan and what the server saw.
#[cfg(feature = "devtriggers")]
fn resolve_jf(info: String, configure: impl FnOnce(&mut ResolveEnv)) -> (Plan, Vec<JfRequest>) {
    let mut ps = PlaybackSession::IDLE;
    let _g = fresh_registry(&mut ps);
    resolve_jf_locked(&ps, info, configure)
}

/// [`resolve_jf`] for a test that already holds the registry guard.
#[cfg(feature = "devtriggers")]
fn resolve_jf_locked(ps: &PlaybackSession, info: String, configure: impl FnOnce(&mut ResolveEnv)) -> (Plan, Vec<JfRequest>) {
    assert!(nj_net::net::global_init());
    let lb = JfLoopback::start(info, user_config(None, true, None, "Default"));
    let rk = jf_rk();
    let mut env = ResolveEnv::snapshot(ps, crate::stores::metadata::MetadataStore::default().view(), lb.sid, &rk);
    let mut track = eac3_track();
    track.id = 2;
    let mut item = fourk_item(lb.sid, vec![track]);
    item.rk = rk.clone();
    env.cached_item = Some(item);
    configure(&mut env);
    let part = format!("/Videos/{JF_GUID}/stream.mkv?static=true&MediaSourceId={JF_GUID}");
    let plan = build_stream(&rk, &part, "hevc", "eac3", &env);
    (plan, lb.finish())
}

#[test]
#[cfg(feature = "devtriggers")]
fn force_registers_original_despite_saved_quality_relay_and_device_raster() {
    let mut ps = PlaybackSession::IDLE;
    let _g = fresh_registry(&mut ps);
    restore_quality(Quality::P480);
    restore_direct_play_mode(DirectPlayMode::Forced);
    let (plan, requests) = resolve_jf_locked(&ps, playback_info(true, "mkv", HEVC_EAC3, None), |env| {
        crate::catalog::client_for(env.sid).unwrap().set_link(crate::catalog::probe::Location::Relay);
        if let Some(item) = env.cached_item.as_mut() {
            item.width = 7680;
            item.height = 4320;
        }
    });
    assert!(plan.url.contains(&format!("/Videos/{JF_GUID}/stream.mkv?static=true")), "{}", plan.url);
    assert!(plan.tsession.is_empty() && plan.contract.ceiling.is_none() && !plan.contract.remux);
    assert!(plan.auto_original.is_none() && !plan.auto_original_watched);
    assert_eq!(plan.direct_play_mode, DirectPlayMode::Forced);
    let body = playback_info_body(&requests);
    assert_eq!(body["EnableDirectPlay"], true);
    assert_eq!(body["AllowVideoStreamCopy"], false);
    assert_eq!(body["MaxStreamingBitrate"].as_i64(), Some(200_000_000), "Force asks for the original, unbounded");
    assert_eq!(quality(), Quality::P480, "Force must retain the saved ceiling");
    restore_quality(Quality::Original);
    restore_direct_play_mode(DirectPlayMode::Auto);
}

#[test]
#[cfg(feature = "devtriggers")]
fn force_server_refusal_or_a_conversion_answer_never_plays_a_conversion() {
    let url = format!("/videos/{JF_GUID}/master.m3u8?VideoCodec=h264&AudioCodec=aac&TranscodeReasons=VideoCodecNotSupported");
    for info in [
        playback_info(false, "mkv", HEVC_EAC3, Some(&url)),
        r#"{"MediaSources":[],"ErrorCode":"NotAllowed"}"#.to_string(),
    ] {
        let (plan, requests) = resolve_jf(info, |env| env.direct_play_mode = DirectPlayMode::Forced);
        assert!(plan.url.is_empty() && plan.tsession.is_empty());
        assert!(plan.verdict.as_ref().unwrap().text().contains("Force Direct Play is on"));
        assert_eq!(requests.iter().filter(|r| r.line.contains("/PlaybackInfo")).count(), 1);
    }
}

#[test]
#[cfg(feature = "devtriggers")]
fn disabling_direct_play_keeps_codec_preserving_remux() {
    let url = format!("/videos/{JF_GUID}/stream.mkv?VideoCodec=hevc&AudioCodec=eac3&TranscodeReasons=ContainerNotSupported");
    let (plan, requests) = resolve_jf(playback_info(false, "mkv", HEVC_EAC3, Some(&url)), |env| {
        env.direct_play_mode = DirectPlayMode::Disabled;
    });
    let body = playback_info_body(&requests);
    assert_eq!(body["EnableDirectPlay"], false);
    assert_eq!(body["AllowVideoStreamCopy"], true);
    assert!(plan.contract.remux && !plan.tsession.is_empty(), "{}", plan.url);
}

#[test]
fn force_retains_feed_limits_and_session_snapshot_across_retry_and_track_edits() {
    let mut ps = PlaybackSession::IDLE;
    let _g = fresh_registry(&mut ps);
    ps.direct_play_mode = DirectPlayMode::Forced;
    restore_direct_play_mode(DirectPlayMode::Disabled);
    assert!(forced_direct_play(&ps));
    assert_eq!(current_retry_context(&ps, 123).direct_play_mode, DirectPlayMode::Forced);
    assert!(forced_direct_play(&ps.publication()));
    assert!(audio_track_direct_plays(&ps, "dts", 8), "Force bypasses device channels");
    assert!(!audio_track_direct_plays(&ps, "truehd", 8));
    assert!(!audio_track_direct_plays(&ps, "", 0));
    assert!(!video_feed_supported("vp9", crate::metadata::DvPresentation::NotDv));
    let blocked_dv = crate::metadata::Dovi { present: true, profile: 5, bl_compat: 0, ..crate::metadata::Dovi::NONE }
        .presentation(false, nj_platform::devcaps::dv::DvCapability::Unsupported, true);
    assert!(!video_feed_supported("hevc", blocked_dv));
    assert!(hls_abr_control(&ps).is_none());
    assert!(auto_original_watch(&ps).is_none());
    assert!(fallback_auto_to_hls(&mut ps, 1000, 0).is_none());
    assert!(transcode_seek(&mut ps, 0).is_none());
    assert!(recover_auto_to_original(&mut ps, 0).is_none());
    commit_audio_selection(&mut ps, CarriedAudio { sid: 9, ordinal: 0, codec: "truehd".into(), channels: 8, can_normalize_loudness: false, immersive: false });
    assert!(play_verdict(&ps).unwrap().contains("Force Direct Play is on"));
    assert_eq!(cur_audio_sid(&ps), 0, "refusing an unsupported track leaves the current selection intact");
    restore_direct_play_mode(DirectPlayMode::Auto);
}

#[test]
fn disabled_mode_refuses_an_original_only_url_without_a_pms_item() {
    let mut ps = PlaybackSession::IDLE;
    let _g = fresh_registry(&mut ps);
    let mut env = ResolveEnv::snapshot(&ps, crate::stores::metadata::MetadataStore::default().view(), unregistered_sid(), "");
    env.direct_play_mode = DirectPlayMode::Disabled;
    let plan = build_stream("", "/movie.mkv", "h264", "aac", &env);
    assert!(plan.url.is_empty());
    assert_eq!(plan.verdict, Some(PlayVerdict::DirectPlayDisabled));
}

/// **Switch to Auto and play** (the failure read-out's fix for Force): a plain retry repeats the
/// failed attempt under Force — the same request, the same refusal — so the fix must resolve the
/// retry under the override, whatever the failed attempt's own snapshot says.
#[test]
fn the_play_automatically_retry_resolves_under_auto_not_the_failed_force() {
    let mut ps = PlaybackSession::IDLE;
    let _g = fresh_registry(&mut ps);
    ps.direct_play_mode = DirectPlayMode::Forced;
    assert_eq!(retry_context_with(&ps, 123, None).direct_play_mode, DirectPlayMode::Forced,
        "an ordinary retry is the SAME request");
    assert_eq!(retry_context_with(&ps, 123, Some(DirectPlayMode::Auto)).direct_play_mode, DirectPlayMode::Auto);
    assert_eq!(retry_context_with(&ps, 123, Some(DirectPlayMode::Auto)).resume_ns, 123);
}
