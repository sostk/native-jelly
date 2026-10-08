//! Plan tests: plan round-trips, and `build_stream` against a loopback Jellyfin — the
//! PlaybackInfo it asks, and the plan it builds from each answer.

use super::*;
#[allow(unused_imports)]
use super::test_support::*;
use super::test_support::apply_plan;

/// The identity round trip: `request_play`'s captured id reaches `cur_sid` unchanged, through
/// `ResolveEnv` (the main-thread snapshot) and `Plan` (the worker's output).
///
/// Graded on the resolve that FAILS, deliberately — it is the exit `build_stream` takes first,
/// before any network, and a plan that carries no server is one `apply_plan` cannot install an
/// honest `cur_sid` from. Every richer exit builds on the same field.
#[test]
#[cfg(feature = "devtriggers")]
fn a_plan_round_trips_the_server_the_request_captured() {
    let mut ps = crate::route::PlaybackSession::IDLE;
    let _g = fresh_registry(&mut ps);
    let sid = unregistered_sid();

    let env = ResolveEnv::snapshot(&ps, crate::stores::metadata::MetadataStore::default().view(), sid, "rk-7");
    assert_eq!(
        env.sid, sid,
        "the snapshot carries the id the request was made with"
    );

    let plan = build_stream("rk-7", "/library/parts/5/1/f.mkv", "h264", "ac3", &env);
    assert_eq!(
        plan.sid, sid,
        "a plan that could not resolve still names its server"
    );
    assert!(
        plan.url.is_empty(),
        "no client for that slot, so nothing resolved"
    );

    apply_plan(&mut ps, plan, "rk-7");
    assert_eq!(cur_sid(&ps), sid, "the installed identity is the captured one");
    assert_eq!(cur_rk(&ps), "rk-7", "and its other half");
}

/// **A plan that never reached the codec gate must not claim the source is undecodable.**
///
/// `Plan::source_decodable` is a `bool`, `bool::default()` is `false`, and `false` is a CLAIM —
/// the quality menu renders it as "Converts on server" on the Original row. `build_stream` has
/// an exit (no client for the server slot) that returns before the gate runs at all, so the
/// value has to be `true` on the way in and be overwritten by evidence, not the other way
/// round.
///
/// Differential: with the initializer's explicit `true` removed this fails, and the failure is
/// a line of copy asserting something about a file nobody opened.
#[test]
fn a_plan_that_never_resolved_makes_no_claim_about_the_source() {
    let mut ps = crate::route::PlaybackSession::IDLE;
    let _g = fresh_registry(&mut ps);
    let sid = unregistered_sid();
    let env = ResolveEnv::snapshot(&ps, crate::stores::metadata::MetadataStore::default().view(), sid, "rk-7");

    let plan = build_stream("rk-7", "/library/parts/5/1/f.mkv", "h264", "ac3", &env);
    assert!(
        plan.url.is_empty(),
        "the test needs the exit that precedes the codec gate"
    );
    assert!(
        plan.source_decodable,
        "nobody looked at this file, so nothing may be said about it",
    );
    apply_plan(&mut ps, plan, "rk-7");
    assert!(
        source_decodable(&ps),
        "and the session carries the same silence"
    );
}

/// Issue #266: `apply_plan` is the ONE place a resolve's audio facts enter the session. The
/// contract's `audio` (what the URL asked for), the outcome (what the decision did with it) and
/// the carried track (what the pipeline feeds) are installed exactly as the plan built them —
/// no fabricated `CarriedAudio` from the flat codec fields, and a plan without a track installs
/// `None` rather than a confident default.
#[test]
fn apply_plan_installs_contract_outcome_audio() {
    let mut ps = crate::route::PlaybackSession::IDLE;
    let _g = fresh_registry(&mut ps);
    let carried = CarriedAudio {
        sid: 31,
        ordinal: 2,
        codec: "truehd".into(),
        channels: 8,
        can_normalize_loudness: true,
        immersive: false,
    };
    let audio = crate::catalog::AudioEnhancements { boost_dialog: true, normalize_loudness: false };
    apply_plan(
        &mut ps,
        Plan {
            url: "https://example.invalid/start.mkv".into(),
            acodec: "ac3".into(),
            src_acodec: "truehd".into(),
            contract: crate::catalog::EncodeContract { audio, ..Default::default() },
            enhancement: EnhancementOutcome::Applied,
            audio: Some(carried.clone()),
            ..Default::default()
        },
        "rk-enh",
    );
    assert_eq!(ps.cur_contract.audio, audio, "the contract carries what the URL asked for");
    assert_eq!(ps.cur_enhancement, EnhancementOutcome::Applied);
    assert_eq!(ps.cur_audio, Some(carried), "the plan's own track, not one rebuilt from acodec");

    // A plan that carried no track (server default, list not fetched) installs no track.
    apply_plan(
        &mut ps,
        Plan {
            url: "https://example.invalid/f.mkv".into(),
            acodec: "eac3".into(),
            src_acodec: "eac3".into(),
            ..Default::default()
        },
        "rk-plain",
    );
    assert_eq!(ps.cur_contract.audio, crate::catalog::AudioEnhancements::NONE);
    assert_eq!(ps.cur_enhancement, EnhancementOutcome::Off);
    assert_eq!(ps.cur_audio, None, "no fabricated track from the flat codec fields");
}

const HEVC_EAC3: &str = r#"{"Type":"Video","Codec":"hevc","Index":0},{"Type":"Audio","Codec":"eac3","Index":1,"Channels":6}"#;

fn jf_part() -> String {
    format!("/Videos/{JF_GUID}/stream.mkv?static=true&MediaSourceId={JF_GUID}")
}

/// An embedded track the way `jf::convert` files it: id = stream index + 1.
fn jf_track(index: i64, codec: &str, channels: i64, lang: &str, default: bool) -> crate::metadata::Stream {
    crate::metadata::Stream {
        id: index + 1,
        index,
        lang_code: lang.into(),
        codec: codec.into(),
        channels,
        default,
        ..Default::default()
    }
}

fn jf_item(
    sid: ServerId,
    audio: Vec<crate::metadata::Stream>,
    subs: Vec<crate::metadata::Stream>,
) -> crate::metadata::PlayingItem {
    let mut item = fourk_item_with_subs(sid, audio, subs);
    item.rk = jf_rk();
    item.width = 1920;
    item.height = 1080;
    item
}

/// One resolve of the loopback item against `info` / `me`, under `configure`; returns the plan
/// and every request the server saw.
fn resolve_jf(
    info: String,
    me: String,
    audio: Vec<crate::metadata::Stream>,
    subs: Vec<crate::metadata::Stream>,
    configure: impl FnOnce(&mut ResolveEnv),
) -> (Plan, Vec<JfRequest>) {
    let mut ps = PlaybackSession::IDLE;
    let _g = fresh_registry(&mut ps);
    assert!(nj_net::net::global_init() && crate::curlio::available());
    restore_quality(Quality::Original);
    let lb = JfLoopback::start(info, me);
    let rk = jf_rk();
    let mut env = ResolveEnv::snapshot(&ps, crate::stores::metadata::MetadataStore::default().view(), lb.sid, &rk);
    let acodec = audio.iter().find(|a| a.default).or(audio.first()).map(|a| a.codec.clone()).unwrap_or_default();
    env.cached_item = Some(jf_item(lb.sid, audio, subs));
    configure(&mut env);
    let plan = build_stream(&rk, &jf_part(), "hevc", &acodec, &env);
    let requests = lb.finish();
    (plan, requests)
}

fn default_user() -> String {
    user_config(None, true, None, "Default")
}

fn eac3_only() -> Vec<crate::metadata::Stream> {
    vec![jf_track(1, "eac3", 6, "eng", true)]
}

#[test]
#[cfg(feature = "devtriggers")]
fn a_direct_play_answer_returns_the_static_stream_url() {
    let (plan, requests) =
        resolve_jf(playback_info(true, "mkv", HEVC_EAC3, None), default_user(), eac3_only(), Vec::new(), |_| {});
    let body = playback_info_body(&requests);
    assert_eq!(body["UserId"], "user-loopback");
    assert_eq!(body["EnableDirectPlay"], true);
    assert_eq!(body["AudioStreamIndex"], 1, "the file's own track, by stream index");
    assert_eq!(body["SubtitleStreamIndex"], -1, "subtitles off");
    assert_eq!(body["AutoOpenLiveStream"], false);
    assert!(plan.url.contains(&format!("/Videos/{JF_GUID}/stream.mkv?static=true")), "{}", plan.url);
    assert!(plan.url.contains("PlaySessionId=ps-loopback"), "{}", plan.url);
    assert!(plan.url.contains("ApiKey=jf-token"), "{}", plan.url);
    assert!(plan.tsession.is_empty(), "direct play has no conversion to stop");
    assert!(plan.verdict.is_none());
    assert_eq!((plan.vcodec.as_str(), plan.acodec.as_str()), ("hevc", "eac3"));
}

/// The ask names the media source the part points at. The server applies `AudioStreamIndex` and
/// `SubtitleStreamIndex` only to the source whose id matches `MediaSourceId`
/// (`MediaInfoHelper.SetDeviceSpecificData`), so an ask without one has its track picks ignored.
#[test]
#[cfg(feature = "devtriggers")]
fn the_playback_info_ask_names_the_parts_media_source() {
    let (_, requests) =
        resolve_jf(playback_info(true, "mkv", HEVC_EAC3, None), default_user(), eac3_only(), Vec::new(), |_| {});
    assert_eq!(playback_info_body(&requests)["MediaSourceId"], JF_GUID);
}

/// An item with several versions answers with several sources. The one played is the one asked
/// for — by id — not whichever the server happened to list first.
#[test]
#[cfg(feature = "devtriggers")]
fn the_answer_source_is_the_one_asked_for_not_merely_the_first() {
    let other = "fedcba9876543210fedcba9876543210";
    let url = format!("/videos/{JF_GUID}/stream.mkv?VideoCodec=h264&AudioCodec=aac&TranscodeReasons=VideoCodecNotSupported&MediaSourceId={other}");
    let info = format!(
        r#"{{"MediaSources":[{{"Id":"{other}","Container":"avi","Protocol":"File","SupportsDirectPlay":false,"SupportsDirectStream":false,"SupportsTranscoding":true,"TranscodingUrl":{url:?},"MediaStreams":[{HEVC_EAC3}]}},{{"Id":"{JF_GUID}","Container":"mkv","Protocol":"File","SupportsDirectPlay":true,"SupportsDirectStream":true,"SupportsTranscoding":true,"MediaStreams":[{HEVC_EAC3}]}}],"PlaySessionId":"ps-loopback"}}"#
    );
    let (plan, _) = resolve_jf(info, default_user(), eac3_only(), Vec::new(), |_| {});
    assert!(plan.url.contains(&format!("stream.mkv?static=true&MediaSourceId={JF_GUID}")), "{}", plan.url);
    assert!(plan.tsession.is_empty(), "the requested version direct-plays: {}", plan.url);
}

#[test]
#[cfg(feature = "devtriggers")]
fn smart_direct_play_asks_for_the_ac3_sibling_of_a_truehd_default() {
    let streams = r#"{"Type":"Video","Codec":"hevc","Index":0},{"Type":"Audio","Codec":"truehd","Index":1,"Channels":8},{"Type":"Audio","Codec":"ac3","Index":2,"Channels":6}"#;
    let audio = vec![jf_track(1, "truehd", 8, "eng", true), jf_track(2, "ac3", 6, "eng", false)];
    let (plan, requests) =
        resolve_jf(playback_info(true, "mkv", streams, None), default_user(), audio, Vec::new(), |_| {});
    let body = playback_info_body(&requests);
    assert_eq!(body["AudioStreamIndex"], 2, "the sibling the panel can decode");
    assert_eq!(plan.acodec, "ac3");
    assert_eq!(plan.audio.as_ref().map(|a| a.sid), Some(3));
}

#[test]
#[cfg(feature = "devtriggers")]
fn a_transcode_answer_plays_the_transcoding_url_in_its_output_codecs() {
    let url = format!(
        "/videos/{JF_GUID}/master.m3u8?VideoCodec=h264&AudioCodec=aac&TranscodeReasons=VideoCodecNotSupported,AudioCodecNotSupported&PlaySessionId=ps-loopback"
    );
    let (plan, requests) =
        resolve_jf(playback_info(false, "mkv", HEVC_EAC3, Some(&url)), default_user(), eac3_only(), Vec::new(), |_| {});
    let _ = playback_info_body(&requests);
    assert!(plan.url.contains(&format!("/videos/{JF_GUID}/master.m3u8?")), "{}", plan.url);
    assert!(plan.url.contains("ApiKey=jf-token"), "{}", plan.url);
    assert_eq!((plan.vcodec.as_str(), plan.acodec.as_str()), ("h264", "aac"));
    assert!(!plan.contract.remux, "a re-encode is not a remux");
    assert!(!plan.tsession.is_empty() && plan.tsession == plan.sess, "the conversion is stoppable by its session");
}

/// Resolve the loopback's conversion with the press's resume, install it, and run the landing's
/// resume to `resume_ns`; returns every request the server saw.
fn resume_jf_conversion(start_ns: i64, resume_ns: i64) -> Vec<JfRequest> {
    let url = format!(
        "/videos/{JF_GUID}/stream.mkv?VideoCodec=h264&AudioCodec=aac&TranscodeReasons=VideoCodecNotSupported&PlaySessionId=ps-loopback"
    );
    let mut ps = PlaybackSession::IDLE;
    let _g = fresh_registry(&mut ps);
    assert!(nj_net::net::global_init() && crate::curlio::available());
    restore_quality(Quality::Original);
    let lb = JfLoopback::start(playback_info(false, "mkv", HEVC_EAC3, Some(&url)), default_user());
    let rk = jf_rk();
    let mut env = ResolveEnv::snapshot(&ps, crate::stores::metadata::MetadataStore::default().view(), lb.sid, &rk);
    env.cached_item = Some(jf_item(lb.sid, eac3_only(), Vec::new()));
    env.start_ns = start_ns;
    let plan = build_stream(&rk, &jf_part(), "hevc", "eac3", &env);
    assert!(!plan.tsession.is_empty(), "the loopback converts: {}", plan.url);
    apply_plan(&mut ps, plan, &rk);
    assert_eq!(crate::player::resume_at(&mut ps, resume_ns), crate::player::ResumeOutcome::Prepared);
    lb.finish()
}

/// A resumed conversion asks the server once. The press knows the resume before the resolve
/// starts, so the first PlaybackInfo already starts the encoder there, and the landing's resume
/// finds it in place instead of negotiating a second encoder and ending the first.
#[test]
#[cfg(feature = "devtriggers")]
fn a_resumed_conversion_is_negotiated_once_at_the_resume() {
    let seen = resume_jf_conversion(754_400_000_000, 754_400_000_000);
    let asks: Vec<_> = seen.iter().filter(|r| r.line.contains("/PlaybackInfo")).collect();
    assert_eq!(asks.len(), 1, "{seen:?}");
    assert_eq!(asks[0].json()["StartTimeTicks"], 754_i64 * 10_000_000);
    assert!(!seen.iter().any(|r| r.line.starts_with("DELETE ")), "no encoder to retire: {seen:?}");
}

/// The encoder only stands in for the resume it was started at. A landing that resumes somewhere
/// else still restarts the conversion there.
#[test]
#[cfg(feature = "devtriggers")]
fn a_landing_resuming_elsewhere_still_restarts_the_conversion() {
    let seen = resume_jf_conversion(754_400_000_000, 100_000_000_000);
    let asks: Vec<_> = seen.iter().filter(|r| r.line.contains("/PlaybackInfo")).collect();
    assert_eq!(asks.len(), 2, "{seen:?}");
    assert_eq!(asks[1].json()["StartTimeTicks"], 100_i64 * 10_000_000);
}

#[test]
#[cfg(feature = "devtriggers")]
fn a_direct_stream_answer_is_a_remux_that_keeps_the_source_codecs() {
    let url = format!(
        "/videos/{JF_GUID}/stream.mkv?VideoCodec=hevc&AudioCodec=eac3&TranscodeReasons=ContainerNotSupported&PlaySessionId=ps-loopback"
    );
    let (plan, _) =
        resolve_jf(playback_info(false, "avi", HEVC_EAC3, Some(&url)), default_user(), eac3_only(), Vec::new(), |_| {});
    assert!(plan.contract.remux, "{}", plan.url);
    assert_eq!((plan.vcodec.as_str(), plan.acodec.as_str()), ("hevc", "eac3"));
    assert!(!plan.tsession.is_empty());
}

#[test]
#[cfg(feature = "devtriggers")]
fn a_playback_info_error_code_is_the_servers_verdict() {
    let info = r#"{"MediaSources":[],"ErrorCode":"NoCompatibleStream"}"#.to_string();
    let (plan, _) = resolve_jf(info, default_user(), eac3_only(), Vec::new(), |_| {});
    assert!(plan.url.is_empty());
    let verdict = plan.verdict.expect("a refusal is a verdict");
    assert_eq!(verdict, PlayVerdict::Server(crate::catalog::Refusal::NoCompatibleStream));
    // The viewer reads a sentence, not the API's enum name.
    assert!(!verdict.text().contains("NoCompatibleStream"), "{}", verdict.text());
}

/// Each `PlaybackErrorCode` the API defines is its own category, and a code this client does not
/// know is still a refusal — worded generically, never echoed.
#[test]
#[cfg(feature = "devtriggers")]
fn every_playback_error_code_is_a_typed_refusal() {
    use crate::catalog::Refusal;
    for (code, expected) in [
        ("NotAllowed", Refusal::NotAllowed),
        ("RateLimitExceeded", Refusal::RateLimitExceeded),
        ("SomethingNew", Refusal::Unrecognized("SomethingNew".into())),
    ] {
        let info = format!(r#"{{"MediaSources":[],"ErrorCode":"{code}"}}"#);
        let (plan, _) = resolve_jf(info, default_user(), eac3_only(), Vec::new(), |_| {});
        let verdict = plan.verdict.expect("a refusal is a verdict");
        assert_eq!(verdict, PlayVerdict::Server(expected), "{code}");
        assert!(!verdict.text().contains(code), "{code}: {}", verdict.text());
    }
    let (plan, _) = resolve_jf(r#"{"MediaSources":[]}"#.to_string(), default_user(), eac3_only(), Vec::new(), |_| {});
    assert_eq!(plan.verdict, Some(PlayVerdict::Server(Refusal::NoMediaSource)));
}

#[test]
#[cfg(feature = "devtriggers")]
fn the_users_audio_language_preference_picks_the_track() {
    let streams = r#"{"Type":"Video","Codec":"hevc","Index":0},{"Type":"Audio","Codec":"ac3","Index":1,"Channels":6,"Language":"eng"},{"Type":"Audio","Codec":"ac3","Index":2,"Channels":6,"Language":"fre"}"#;
    let audio = || vec![jf_track(1, "ac3", 6, "eng", true), jf_track(2, "ac3", 6, "fre", false)];
    for (play_default, expected) in [(false, 2), (true, 1)] {
        let (_, requests) = resolve_jf(
            playback_info(true, "mkv", streams, None),
            user_config(Some("fre"), play_default, None, "Default"),
            audio(),
            Vec::new(),
            |_| {},
        );
        assert_eq!(
            playback_info_body(&requests)["AudioStreamIndex"], expected,
            "PlayDefaultAudioTrack={play_default}"
        );
    }
}

#[test]
#[cfg(feature = "devtriggers")]
fn subtitle_mode_always_turns_on_the_preferred_language() {
    let streams = r#"{"Type":"Video","Codec":"hevc","Index":0},{"Type":"Audio","Codec":"eac3","Index":1,"Channels":6},{"Type":"Subtitle","Codec":"subrip","Index":2,"Language":"eng"}"#;
    let subs = vec![jf_track(2, "srt", 0, "eng", false)];
    let (plan, requests) = resolve_jf(
        playback_info(true, "mkv", streams, None),
        user_config(None, true, Some("eng"), "Always"),
        eac3_only(),
        subs,
        |_| {},
    );
    assert_eq!(playback_info_body(&requests)["SubtitleStreamIndex"], 2);
    assert_eq!(plan.sub_sid, 3, "the client renders it over direct play");
}

#[test]
#[cfg(feature = "devtriggers")]
fn an_unconfirmed_profile_5_withdraws_video_copy() {
    let url = format!("/videos/{JF_GUID}/master.m3u8?VideoCodec=h264&AudioCodec=eac3&TranscodeReasons=VideoCodecNotSupported");
    for capability in [
        nj_platform::devcaps::dv::DvCapability::Unknown,
        nj_platform::devcaps::dv::DvCapability::Unsupported,
    ] {
        let (plan, requests) = resolve_jf(
            playback_info(false, "mkv", HEVC_EAC3, Some(&url)),
            default_user(),
            eac3_only(),
            Vec::new(),
            |env| {
                env.dv_capability = Some(capability);
                if let Some(item) = env.cached_item.as_mut() {
                    item.dovi = p5();
                }
            },
        );
        let body = playback_info_body(&requests);
        assert_eq!(body["AllowVideoStreamCopy"], false, "{capability:?}");
        assert!(!plan.contract.remux, "{capability:?}");
        assert_eq!(plan.dovi, crate::metadata::Dovi::NONE);
        assert_eq!(plan.dv_decision.presentation, crate::metadata::DvPresentation::NotDv);
    }
}

#[test]
#[cfg(feature = "devtriggers")]
fn a_fixed_quality_bounds_max_streaming_bitrate() {
    let (_, requests) = resolve_jf(playback_info(true, "mkv", HEVC_EAC3, None), default_user(), eac3_only(), Vec::new(), |env| {
        env.quality = Quality::P720;
    });
    let body = playback_info_body(&requests);
    let kbps = Quality::P720.ceiling().expect("a fixed rung has a ceiling").max_kbps;
    assert_eq!(body["MaxStreamingBitrate"].as_i64(), Some(kbps * 1000));
    assert!(!requests.iter().any(|r| r.line.contains("/Playback/BitrateTest")), "a fixed rung measures nothing");
}

#[test]
#[cfg(feature = "devtriggers")]
fn remote_auto_measures_the_link_and_bounds_the_ask() {
    let (_, requests) = resolve_jf(playback_info(true, "mkv", HEVC_EAC3, None), default_user(), eac3_only(), Vec::new(), |env| {
        env.quality = Quality::Auto;
        crate::catalog::client_for(env.sid).unwrap().set_link(crate::catalog::probe::Location::Remote);
    });
    let test = requests.iter().position(|r| r.line.contains("/Playback/BitrateTest")).expect("Auto measured the link");
    let info = requests.iter().position(|r| r.line.contains("/PlaybackInfo")).unwrap();
    assert!(test < info, "the measurement bounds the ask, so it comes first");
    let bps = playback_info_body(&requests)["MaxStreamingBitrate"].as_i64().unwrap();
    assert!((1_000_000..50_000_000).contains(&bps), "70% of the ~10 Mbit/s measured: {bps}");
}

#[test]
#[cfg(feature = "devtriggers")]
fn local_auto_asks_unbounded_without_measuring() {
    let (_, requests) = resolve_jf(playback_info(true, "mkv", HEVC_EAC3, None), default_user(), eac3_only(), Vec::new(), |env| {
        env.quality = Quality::Auto;
        crate::catalog::client_for(env.sid).unwrap().set_link(crate::catalog::probe::Location::Local);
    });
    assert!(!requests.iter().any(|r| r.line.contains("/Playback/BitrateTest")));
    assert_eq!(playback_info_body(&requests)["MaxStreamingBitrate"].as_i64(), Some(200_000_000));
}

#[test]
#[cfg(feature = "devtriggers")]
fn a_preview_never_plays_a_conversion() {
    let url = format!("/videos/{JF_GUID}/master.m3u8?VideoCodec=h264&AudioCodec=aac&TranscodeReasons=VideoCodecNotSupported");
    let (plan, requests) = resolve_jf(playback_info(false, "mkv", HEVC_EAC3, Some(&url)), default_user(), eac3_only(), Vec::new(), |env| {
        env.preview = true;
    });
    assert!(plan.url.is_empty() && plan.tsession.is_empty());
    assert!(
        requests.iter().any(|r| r.line.starts_with("DELETE /Videos/ActiveEncodings?")),
        "the conversion the server started for it is ended: {requests:?}"
    );
    assert!(
        !requests.iter().any(|r| r.line.contains("/Sessions/Playing/Stopped")),
        "a preview that never played reports no stop, which would rewrite the resume point: {requests:?}"
    );
}
