//! The session contract a playback keeps with Jellyfin across encoder replacements: what the wire
//! `PlayMethod` says, which media source a re-negotiation names, and what retiring a superseded
//! encoder does — and must not do — to the user's resume point and play count.
//!
//! A seek, a track switch, a quality change and every resumed conversion replace the encoder under
//! a new `PlaySessionId`. The official web client does the same (`changeStream`): it re-asks
//! PlaybackInfo for the CURRENT media source, ends the old job with
//! `DELETE /Videos/ActiveEncodings`, and goes on reporting `/Progress` — the playback never
//! restarts in the server's eyes.

use super::*;
use super::test_support::*;

const HEVC_EAC3: &str = r#"{"Type":"Video","Codec":"hevc","Index":0},{"Type":"Audio","Codec":"eac3","Index":1,"Channels":6}"#;

fn video_transcode_info() -> String {
    let url = format!(
        "/videos/{JF_GUID}/stream.mkv?VideoCodec=h264&AudioCodec=aac&TranscodeReasons=VideoCodecNotSupported&PlaySessionId=ps-loopback"
    );
    playback_info(false, "mkv", HEVC_EAC3, Some(&url))
}

fn remux_info() -> String {
    let url = format!(
        "/videos/{JF_GUID}/stream.mkv?VideoCodec=hevc&AudioCodec=eac3&TranscodeReasons=ContainerNotSupported&PlaySessionId=ps-loopback"
    );
    playback_info(false, "avi", HEVC_EAC3, Some(&url))
}

fn loopback(info: String) -> JfLoopback {
    assert!(nj_net::net::global_init() && crate::curlio::available());
    JfLoopback::start(info, user_config(None, true, None, "Default"))
}

fn encode(lb: &JfLoopback, session: &str, continues: &str, offset_secs: i64, contract: crate::catalog::EncodeContract) {
    let client = crate::catalog::client_for(lb.sid).expect("loopback registered");
    let rk = jf_rk();
    let spec = transcode_spec(
        &rk,
        session,
        session,
        continues,
        crate::catalog::TranscodeOffset::from_seconds(offset_secs),
        2,
        0,
        contract,
    );
    assert!(
        matches!(client.transcode(&spec), crate::catalog::Negotiation::Playable(_)),
        "the loopback negotiates {session}"
    );
}

fn report(lb: &JfLoopback, session: &str, state: crate::catalog::TimelineState, time_ms: i64) {
    let client = crate::catalog::client_for(lb.sid).expect("loopback registered");
    let rk = jf_rk();
    assert!(client.timeline(&crate::catalog::TimelineReport {
        rating_key: &rk,
        state,
        time_ms,
        duration_ms: 7_200_000,
        session,
        play_queue_id: "",
        play_queue_item_id: "",
        audio_stream_id: 2,
        subtitle_stream_id: 0,
    }));
}

fn posts<'a>(requests: &'a [JfRequest], path: &str) -> Vec<&'a JfRequest> {
    let line = format!("POST {path} ");
    let query = format!("POST {path}?");
    requests.iter().filter(|r| r.line.starts_with(&line) || r.line.starts_with(&query)).collect()
}

/// A remux is ffmpeg copying streams into a new container from the `TranscodingUrl`. The server
/// itself marks every TranscodingUrl playback `Transcode` (`MediaInfoHelper`), the web client
/// reports it so, and the dashboard derives "Remux" from `TranscodingInfo` — which the server
/// CLEARS on every report whose method is not `Transcode`.
#[test]
fn a_remux_reports_transcode_on_the_wire() {
    let mut ps = PlaybackSession::IDLE;
    let _g = fresh_registry(&mut ps);
    let lb = loopback(remux_info());
    let contract = crate::catalog::EncodeContract { remux: true, ..Default::default() };
    encode(&lb, "jf-remux", "", 0, contract);
    report(&lb, "jf-remux", crate::catalog::TimelineState::Playing, 1_000);
    let seen = lb.finish();
    let playing = posts(&seen, "/Sessions/Playing");
    assert_eq!(playing.len(), 1, "{seen:?}");
    assert_eq!(playing[0].json()["PlayMethod"], "Transcode");
}

/// Retiring the encoder a seek replaced must end ITS job and nothing else. A `Stopped` report is
/// the wrong tool: the server writes its `PositionTicks` into the user's resume point, so the old
/// zero-position stop rewound "Continue Watching" to the start on every resumed conversion, and it
/// told every plugin the film had ended.
#[test]
fn retiring_a_replaced_encoder_ends_its_job_without_reporting_the_item_stopped() {
    let mut ps = PlaybackSession::IDLE;
    let _g = fresh_registry(&mut ps);
    let lb = loopback(video_transcode_info());
    let contract = crate::catalog::EncodeContract { no_video_copy: true, ..Default::default() };
    encode(&lb, "jf-seek-abr-1", "", 0, contract);
    report(&lb, "jf-seek-abr-1", crate::catalog::TimelineState::Playing, 600_000);
    encode(&lb, "jf-seek-abr-2", "jf-seek-abr-1", 1_200, contract);

    let client = crate::catalog::client_for(lb.sid).expect("loopback registered");
    assert!(client.transcode_stop("jf-seek-abr-1"));
    let seen = lb.finish();

    assert!(posts(&seen, "/Sessions/Playing/Stopped").is_empty(), "no stop for a playback still running: {seen:?}");
    let delete = seen
        .iter()
        .find(|r| r.line.starts_with("DELETE /Videos/ActiveEncodings?"))
        .unwrap_or_else(|| panic!("the old job was never ended: {seen:?}"));
    assert_eq!(query_param(&delete.line, "playSessionId"), Some("ps-loopback"), "{}", delete.line);
    assert!(query_param(&delete.line, "deviceId").is_some_and(|d| !d.is_empty()), "{}", delete.line);
}

/// The replacement is the SAME playback: it reports `/Progress`, not a second `/Sessions/Playing`
/// (which the server counts as another play), and it carries the start stamp of the first.
#[test]
fn a_replacement_encoder_continues_the_playback_it_replaced() {
    let mut ps = PlaybackSession::IDLE;
    let _g = fresh_registry(&mut ps);
    let lb = loopback(video_transcode_info());
    let contract = crate::catalog::EncodeContract { no_video_copy: true, ..Default::default() };
    encode(&lb, "jf-cont-abr-1", "", 0, contract);
    report(&lb, "jf-cont-abr-1", crate::catalog::TimelineState::Playing, 600_000);
    encode(&lb, "jf-cont-abr-2", "jf-cont-abr-1", 1_200, contract);
    let client = crate::catalog::client_for(lb.sid).expect("loopback registered");
    let _ = client.transcode_stop("jf-cont-abr-1");
    report(&lb, "jf-cont-abr-2", crate::catalog::TimelineState::Playing, 1_210_000);
    let seen = lb.finish();

    let playing = posts(&seen, "/Sessions/Playing");
    assert_eq!(playing.len(), 1, "one playback, one start: {seen:?}");
    let progress = posts(&seen, "/Sessions/Playing/Progress");
    assert_eq!(progress.len(), 1, "{seen:?}");
    assert_eq!(
        progress[0].json()["PlaybackStartTimeTicks"],
        playing[0].json()["PlaybackStartTimeTicks"],
        "the start stamp belongs to the playback, not to the encoder"
    );
}

/// A re-negotiation names the media source the playback is on. Without `MediaSourceId` the
/// server applies `AudioStreamIndex`/`SubtitleStreamIndex` to no source at all
/// (`MediaInfoHelper.SetDeviceSpecificData` matches them by source id), so a track switch on a
/// conversion played the default track.
#[test]
fn a_replacement_names_the_media_source_of_the_playback_it_replaces() {
    let mut ps = PlaybackSession::IDLE;
    let _g = fresh_registry(&mut ps);
    let lb = loopback(video_transcode_info());
    let contract = crate::catalog::EncodeContract { no_video_copy: true, ..Default::default() };
    encode(&lb, "jf-src-abr-1", "", 0, contract);
    encode(&lb, "jf-src-abr-2", "jf-src-abr-1", 30, contract);
    let seen = lb.finish();
    let asks: Vec<_> = seen.iter().filter(|r| r.line.contains("/PlaybackInfo")).map(|r| r.json()).collect();
    assert_eq!(asks.len(), 2);
    assert_eq!(asks[1]["MediaSourceId"], JF_GUID, "{}", asks[1]);
    assert_eq!(asks[1]["AudioStreamIndex"], 1);
}

/// Without `MediaSourceId` the server applies neither track index (`SetDeviceSpecificData`), so
/// "subtitles off" went unheard: it embedded the item's default subtitle, a PGS track its ffmpeg
/// then failed to encode (live 12.0, 2026-10-08). A conversion that continues no known playback
/// names the item's own source, whose id is the item's.
#[test]
fn a_conversion_that_continues_nothing_still_names_the_items_source() {
    let mut ps = PlaybackSession::IDLE;
    let _g = fresh_registry(&mut ps);
    let lb = loopback(video_transcode_info());
    let contract = crate::catalog::EncodeContract { no_video_copy: true, ..Default::default() };
    encode(&lb, "jf-fresh", "", 30, contract);
    let seen = lb.finish();
    let ask = seen.iter().find(|r| r.line.contains("/PlaybackInfo")).map(|r| r.json()).expect("asked");
    assert_eq!(ask["MediaSourceId"], JF_GUID, "{ask}");
    assert_eq!(ask["SubtitleStreamIndex"], -1, "{ask}");
}

/// An HLS contract (Auto's rungs) asks for an HLS conversion and plays the server's master as it
/// is. Jellyfin's master lists every segment from zero and has no `StartTimeTicks` — the player
/// starts at the segment covering the offset — and a segment request carrying `StartTimeTicks`
/// is refused by the server, so the offset is never appended. The video is re-encoded: a copy is
/// cut at the source's keyframes, which the playlist's durations follow only when the server could
/// extract them, and the player's timeline is built from those durations.
#[test]
fn an_hls_contract_asks_for_hls_and_keeps_the_masters_own_url() {
    let mut ps = PlaybackSession::IDLE;
    let _g = fresh_registry(&mut ps);
    let url = format!(
        "/videos/{JF_GUID}/master.m3u8?VideoCodec=h264&AudioCodec=aac&SegmentContainer=ts&SegmentLength=2&MinSegments=1&TranscodeReasons=ContainerBitrateExceedsLimit&PlaySessionId=ps-loopback"
    );
    let lb = loopback(playback_info(false, "mkv", HEVC_EAC3, Some(&url)));
    let client = crate::catalog::client_for(lb.sid).expect("loopback registered");
    let rk = jf_rk();
    let contract = crate::catalog::EncodeContract {
        delivery: crate::catalog::TranscodeDelivery::FixedHls { seconds_per_segment: 2 },
        ceiling: Some(crate::catalog::Ceiling { max_kbps: 4000, max_w: 1280, max_h: 720 }),
        ..Default::default()
    };
    let spec = transcode_spec(&rk, "jf-hls", "jf-hls", "", crate::catalog::TranscodeOffset::from_seconds(600), 2, 0, contract);
    let n = match client.transcode(&spec) {
        crate::catalog::Negotiation::Playable(n) => n,
        _ => panic!("the loopback negotiates"),
    };
    let seen = lb.finish();

    assert!(n.url.contains("/master.m3u8?"), "{}", n.url);
    assert!(query_param(&n.url, "StartTimeTicks").is_none(), "{}", n.url);
    let ask = seen.iter().find(|r| r.line.contains("/PlaybackInfo")).map(|r| r.json()).expect("asked");
    assert_eq!(ask["StartTimeTicks"], 6_000_000_000i64, "the server still learns where playback is");
    assert_eq!(ask["AllowVideoStreamCopy"], false, "{ask}");
    assert_eq!(ask["EnableDirectStream"], false, "{ask}");
    let t = &ask["DeviceProfile"]["TranscodingProfiles"][0];
    assert_eq!((t["Protocol"].as_str(), t["Container"].as_str()), (Some("hls"), Some("ts")), "{t}");
    assert_eq!(t["SegmentLength"], 2, "{t}");
    assert_eq!(t["MinSegments"], 1, "{t}");
}

/// `info` for a remote Http source whose fetch needs `headers` (`RequiredHttpHeaders`).
fn remote_info(headers: &str, direct: bool, url: Option<&str>) -> String {
    playback_info(direct, "mkv", HEVC_EAC3, url).replacen(
        r#""Protocol":"File""#,
        &format!(r#""Protocol":"Http","IsRemote":true,"Path":"https://media.invalid/a.mkv","RequiredHttpHeaders":{headers}"#),
        1,
    )
}

fn negotiate_direct(lb: &JfLoopback) -> crate::catalog::Negotiated {
    let client = crate::catalog::client_for(lb.sid).expect("loopback registered");
    let rk = jf_rk();
    match client.negotiate(&crate::catalog::PlaybackAsk {
        rk: &rk,
        session: "jf-remote",
        media_source_id: None,
        audio_index: None,
        subtitle_index: None,
        start_ticks: 0,
        ceiling: None,
        direct_play: true,
        video_copy: true,
        forced: false,
        burn: false,
        hls_segment_secs: None,
    }) {
        crate::catalog::Negotiation::Playable(n) => n,
        _ => panic!("the loopback negotiates"),
    }
}

/// The server fetches a remote source itself, and its static proxy forwards only `User-Agent` of
/// the source's `RequiredHttpHeaders` (`FileStreamResponseHelpers.GetStaticRemoteStreamResult`);
/// ffmpeg is given `User-Agent` and `Referer`. A source that needs a `Referer` is therefore asked
/// again without direct play, and played from the server's conversion — never from its own `Path`.
#[test]
fn a_remote_source_needing_a_header_the_static_proxy_drops_plays_through_ffmpeg() {
    let mut ps = PlaybackSession::IDLE;
    let _g = fresh_registry(&mut ps);
    let headers = r#"{"User-Agent":"ua","Referer":"https://ref.invalid/"}"#;
    let lb = loopback(remote_info(headers, true, None));
    let url = format!("/videos/{JF_GUID}/stream.mkv?VideoCodec=hevc&AudioCodec=eac3&TranscodeReasons=DirectPlayError&PlaySessionId=ps-loopback");
    lb.answer_converted(&remote_info(headers, false, Some(&url)));
    let n = negotiate_direct(&lb);
    let seen = lb.finish();

    assert_eq!(n.method, crate::catalog::PlayMethod::Transcode, "{}", n.url);
    assert!(n.url.contains("/videos/") && !n.url.contains("static=true"), "{}", n.url);
    assert!(!n.url.contains("media.invalid"), "{}", n.url);
    let asks: Vec<_> = seen.iter().filter(|r| r.line.contains("/PlaybackInfo")).map(|r| r.json()).collect();
    assert_eq!(asks.len(), 2, "{seen:?}");
    assert_eq!(asks[1]["EnableDirectPlay"], false, "{}", asks[1]);
}

/// A `User-Agent` alone is one the static proxy does send, so such a source still plays directly.
#[test]
fn a_remote_source_needing_only_a_user_agent_still_plays_directly() {
    let mut ps = PlaybackSession::IDLE;
    let _g = fresh_registry(&mut ps);
    let lb = loopback(remote_info(r#"{"User-Agent":"ua"}"#, true, None));
    let n = negotiate_direct(&lb);
    let seen = lb.finish();

    assert_eq!(n.method, crate::catalog::PlayMethod::DirectPlay, "{}", n.url);
    assert!(n.url.contains("static=true") && !n.url.contains("media.invalid"), "{}", n.url);
    assert_eq!(seen.iter().filter(|r| r.line.contains("/PlaybackInfo")).count(), 1, "{seen:?}");
}

// ---- subtitles on a conversion (W1) ---------------------------------------------------------

const HEVC_EAC3_SRT: &str = r#"{"Type":"Video","Codec":"hevc","Index":0},{"Type":"Audio","Codec":"eac3","Index":1,"Channels":6},{"Type":"Subtitle","Codec":"subrip","Index":2,"Language":"eng","DeliveryMethod":"External","DeliveryUrl":"/Videos/0123456789abcdef0123456789abcdef/0123456789abcdef0123456789abcdef/Subtitles/2/0/Stream.srt?api_key=jf-token"}"#;

/// A remux whose text subtitle the server delivers as its own extracted file.
fn remux_with_external_subtitle_info() -> String {
    let url = format!(
        "/videos/{JF_GUID}/stream.mkv?VideoCodec=hevc&AudioCodec=eac3&TranscodeReasons=ContainerNotSupported&PlaySessionId=ps-loopback"
    );
    playback_info(false, "avi", HEVC_EAC3_SRT, Some(&url))
}

/// Picking a subtitle during a conversion re-negotiates WITHOUT forcing a burn: the server
/// delivers the text track as a file, the video stays copied, the new encoder replaces the old,
/// and the client draws the server's file.
#[test]
fn a_subtitle_picked_during_a_conversion_is_delivered_softly_not_burned() {
    let mut ps = PlaybackSession::IDLE;
    let _g = fresh_registry(&mut ps);
    crate::player::reset_subtitle();
    let lb = loopback(remux_with_external_subtitle_info());
    let rk = jf_rk();
    let contract = crate::catalog::EncodeContract { remux: true, ..Default::default() };
    negotiate_jf_session(lb.sid, &rk, "jf-subs-old", contract);
    super::test_support::apply_plan(&mut ps,
        Plan {
            sid: lb.sid,
            sess: "jf-subs-logical".into(),
            tsession: "jf-subs-old".into(),
            url: format!("http://127.0.0.1/videos/{JF_GUID}/stream.mkv?PlaySessionId=ps-loopback"),
            contract,
            vcodec: "hevc".into(),
            acodec: "eac3".into(),
            ..Default::default()
        },
        &rk,
    );
    assert_eq!(cur_sub_sid(&ps), 0);

    // The viewer picks the embedded English text track (stream index 2, app id 3).
    commit_subtitle_selection(&mut ps, 0, 3, true);
    let expected = worker_ticket();
    assert!(retranscode_for(&mut ps, &expected, 90).is_some(), "the pick re-negotiates");
    let seen = lb.finish();

    let ask = seen.iter().filter(|r| r.line.contains("/PlaybackInfo")).last().expect("asked").json();
    assert_eq!(ask["SubtitleStreamIndex"], 2);
    assert_eq!(ask["AlwaysBurnInSubtitleWhenTranscoding"], false, "a pick is not a burn");
    assert_eq!(ask["AllowVideoStreamCopy"], true, "a soft subtitle keeps the video copyable");
    assert_ne!(transcode_session(&ps), "jf-subs-old", "the new encoder replaces the old");
    assert_eq!(
        ps.cur_sub_delivery,
        Some(crate::catalog::SubtitleDelivery::External {
            path: format!("/Videos/{JF_GUID}/{JF_GUID}/Subtitles/2/0/Stream.srt"),
            codec: "srt".into(),
        }),
    );
    assert!(client_renders_subtitle(&ps));
    assert!(crate::player::sidecar::selected());

    crate::player::reset_subtitle();
    reset_session(&mut ps);
    install_active_encoder("");
}

/// An external delivery the client cannot fetch through the server (a remote source's own
/// subtitle URL) is asked again as a burn, rather than selected and never drawn.
#[test]
fn an_external_subtitle_outside_the_server_is_asked_again_as_a_burn() {
    let mut ps = PlaybackSession::IDLE;
    let _g = fresh_registry(&mut ps);
    let streams = r#"{"Type":"Video","Codec":"hevc","Index":0},{"Type":"Audio","Codec":"eac3","Index":1,"Channels":6},{"Type":"Subtitle","Codec":"subrip","Index":2,"IsExternal":true,"DeliveryMethod":"External","DeliveryUrl":"https://subs.example.invalid/film.srt","IsExternalUrl":true}"#;
    let url = format!("/videos/{JF_GUID}/stream.mkv?VideoCodec=hevc&AudioCodec=eac3&TranscodeReasons=ContainerNotSupported&PlaySessionId=ps-loopback");
    let lb = loopback(playback_info(false, "avi", streams, Some(&url)));
    let client = crate::catalog::client_for(lb.sid).expect("loopback registered");
    let rk = jf_rk();
    let contract = crate::catalog::EncodeContract { remux: true, ..Default::default() };
    let spec = transcode_spec(&rk, "jf-remote-sub", "jf-remote-sub", "", crate::catalog::TranscodeOffset::from_seconds(0), 0, 3, contract);
    let n = client.transcode(&spec).playable().expect("playable");
    let seen = lb.finish();

    let asks: Vec<_> = seen.iter().filter(|r| r.line.contains("/PlaybackInfo")).map(|r| r.json()).collect();
    assert_eq!(asks.len(), 2, "{seen:?}");
    assert_eq!(asks[0]["AlwaysBurnInSubtitleWhenTranscoding"], false);
    assert_eq!(asks[1]["AlwaysBurnInSubtitleWhenTranscoding"], true);
    assert_eq!(n.subtitle, Some(crate::catalog::SubtitleDelivery::Burned));
}
