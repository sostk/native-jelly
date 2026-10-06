//! Playback against Jellyfin: the `POST /Items/{id}/PlaybackInfo` negotiation, the URL it yields,
//! and the session reports that follow.
//!
//! **The protocol this implements is Jellyfin's own.** A client states what it can decode as a
//! `DeviceProfile`; the server answers per media source with `SupportsDirectPlay`,
//! `SupportsDirectStream` and `SupportsTranscoding`, plus a `TranscodingUrl` when it would convert
//! and `TranscodeReasons` saying why. The three outcomes are the documented `PlayMethod` values:
//!
//! * `DirectPlay` — the file's own bytes, `GET /Videos/{id}/stream.{ext}?static=true`.
//! * `DirectStream` — the video is copied; the container and/or audio change. Jellyfin's docs call
//!   a both-streams-copied variant "Remux" and an audio-only conversion "Direct Stream", but the
//!   wire enum has one value for both, so anything with the video copied reports as `DirectStream`.
//! * `Transcode` — the video is re-encoded.
//!
//! `PlaySessionId` is the server's handle for the negotiated playback. Everything after the
//! decision is keyed by it: the start URL, `/Sessions/Playing{,/Progress,/Stopped}`, and — since
//! 12.0 has no `/Videos/ActiveEncodings` route — ending the encoder. The decision stores that id
//! (and the source id and track indexes it resolved) in a table keyed by the app's own session
//! string, so the later calls report the same playback the server registered.
//!
//! **Why the return type is a `plex::MediaContainer`.** This module is the Jellyfin half of a
//! facade whose other half talks to Plex Media Server, and the shared routing layer
//! (`route::plan::build_stream`) grades one decision shape for both. So each decision below ends
//! by translating Jellyfin's answer into that shape — a `Part.decision` of `directplay` or
//! `transcode` and a per-lane `decision`/`codec` pair carrying the OUTPUT codecs. The translation
//! is confined to [`verdict`] and [`decision_stream`]; everything above them is Jellyfin's
//! protocol as documented.
use super::models::*;
use super::{api::Jf, convert, ids, ticks};
use crate::plex::{
    Ceiling, MediaContainer, Media, MediaPart, Metadata, Stream, StreamSelection, StreamUrl,
    TimelineReport, TimelineState, TranscodeOffset, TranscodeSpec,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// The bitrate bound to advertise when the user has asked for no ceiling.
///
/// `DeviceProfile.MaxStaticBitrate` defaults to 8 Mbit/s on the server, which would refuse direct
/// play of any remux — so an unconstrained ask has to state its own, and on the LAN this app is
/// built for there is no honest bound to state. A ceiling the user DID pick is applied to both
/// rates (see [`device_profile`]), which is what makes a low rung mean something for the original
/// file as well as for the re-encode.
const UNBOUNDED_BPS: i64 = 200_000_000;
/// Decision codes for the shared routing layer's verdict container (see the module doc): playable,
/// and "neither direct play nor conversion is available".
const DECISION_OK: i64 = 1000;
const DECISION_UNPLAYABLE: i64 = 2000;

/// `PlayMethod`, the three values 12.0's API defines. Reported on every session report, and read
/// back by [`Jf::transcode_stop`] to know whether there is an encoder to end.
const DIRECT_PLAY: &str = "DirectPlay";
const DIRECT_STREAM: &str = "DirectStream";
const TRANSCODE: &str = "Transcode";

/// Which `PlayMethod` a decision landed on, from the two facts that decide it: the server offered
/// the file as-is, or the video lane is copied into a new container/audio pairing.
fn play_method(direct_play: bool, video_copied: bool) -> &'static str {
    match (direct_play, video_copied) {
        (true, _) => DIRECT_PLAY,
        (false, true) => DIRECT_STREAM,
        (false, false) => TRANSCODE,
    }
}

#[derive(Debug, Clone, Default)]
struct Session {
    item_guid: String,
    media_source_id: String,
    play_session_id: String,
    /// `DirectPlay | DirectStream | Transcode`.
    play_method: &'static str,
    transcoding_url: String,
    audio_index: Option<i64>,
    subtitle_index: Option<i64>,
    /// `/Sessions/Playing` has been sent; later reports are `/Progress`.
    started: bool,
    /// `PlaybackStartTimeTicks` — UTC of the first report for this playback, held so every later
    /// report states the same start rather than re-deriving a drifting one.
    start_time_ticks: i64,
}

fn sessions() -> &'static Mutex<HashMap<String, Session>> {
    static T: OnceLock<Mutex<HashMap<String, Session>>> = OnceLock::new();
    T.get_or_init(Default::default)
}

fn session(key: &str) -> Option<Session> {
    sessions().lock().ok()?.get(key).cloned()
}

fn put_session(key: &str, s: Session) {
    if let Ok(mut t) = sessions().lock() {
        // A long run of plays must not grow the table without bound: sessions are short-lived
        // and a stale one is only a missing PlaySessionId on a report.
        if t.len() > 64 {
            t.clear();
        }
        t.insert(key.to_string(), s);
    }
}

fn update_session(key: &str, f: impl FnOnce(&mut Session)) {
    if let Ok(mut t) = sessions().lock() {
        if let Some(s) = t.get_mut(key) {
            f(s);
        }
    }
}

fn drop_session(key: &str) -> Option<Session> {
    sessions().lock().ok()?.remove(key)
}

/// Jellyfin's codec spelling for one of the pipeline's direct-play audio codecs.
fn jf_audio(c: &str) -> &str {
    if c == "dca" { "dts" } else { c }
}

/// Subtitle formats the client draws itself out of the container, in Jellyfin's spelling. These go
/// up as `Method: Embed`, which tells the server to leave the track in the stream.
const EMBED_SUBS: [&str; 10] =
    ["srt", "subrip", "ass", "ssa", "pgssub", "dvdsub", "dvbsub", "mov_text", "webvtt", "vtt"];
/// Text formats the client can fetch as a sidecar (`Method: External`), so the server neither
/// muxes them nor burns them in.
const EXTERNAL_SUBS: [&str; 5] = ["srt", "subrip", "ass", "ssa", "vtt"];
/// Bitmap subtitles. They have no text representation, so `External` is not available for them and
/// the only soft delivery is `Embed`; anything else makes the server burn them into the video.
const IMAGE_SUBS: [&str; 3] = ["pgssub", "dvdsub", "dvbsub"];

pub(crate) struct ProfileAsk {
    /// Offer direct play (and the Embed/External subtitle methods that go with it).
    pub direct: bool,
    /// Strict Original: the software feed's formats, no device limits, no transcode target.
    pub forced: bool,
    /// The bound this playback may not exceed; `None` advertises no bound (see [`UNBOUNDED_BPS`]).
    pub ceiling: Option<Ceiling>,
    /// Burn the selected subtitle into the video. The caller asks for this deliberately — a burn
    /// is the server's most expensive option and the one the subtitle profiles below exist to
    /// avoid, so it is never inferred from "a subtitle is selected".
    pub burn: bool,
}

/// The DeviceProfile for this device — the Jellyfin twin of `transcoder::profile_for_delivery`,
/// derived from the same capability snapshot so the two servers are told the same limits.
pub(crate) fn device_profile(caps: &plx_platform::devcaps::Caps, ask: &ProfileAsk) -> Value {
    let dp_video = if caps.hevc || ask.forced { "h264,hevc" } else { "h264" };
    let dp_audio: Vec<&str> = if ask.forced {
        plx_platform::devcaps::DP_AUDIO_CODECS.split(',').collect()
    } else {
        caps.audio.split(',').filter(|c| !c.is_empty()).map(jf_audio).collect()
    };
    let target_video = match caps.encode_vcodec() {
        "h264" => "h264".to_string(),
        head => format!("{head},h264"),
    };
    let target_audio: Vec<&str> = ["ac3", "eac3", "aac", "dts"].into_iter().filter(|c| caps.audio_has(c)).collect();
    let (mut w, mut h) = caps.hevc_max;
    // Both rates, not just the streaming one. `MaxStreamingBitrate` bounds a re-encode;
    // `MaxStaticBitrate` is what the server weighs direct play against. Leaving the latter
    // unbounded under an explicit ceiling let a 40 Mbit/s remux direct-play while the user had
    // asked for "720p · 3 Mbps" — the ask was honoured for the encode branch and silently ignored
    // for the branch that actually ships the most bytes.
    let mut max_streaming_bps = UNBOUNDED_BPS;
    let mut max_static_bps = UNBOUNDED_BPS;
    if let Some(c) = ask.ceiling {
        w = w.min(c.max_w as u32);
        h = h.min(c.max_h as u32);
        max_streaming_bps = c.max_kbps.saturating_mul(1000);
        max_static_bps = max_streaming_bps;
    }
    let direct_play = if ask.direct {
        vec![json!({
            "Container": "mkv,mp4,m4v,mov",
            "Type": "Video",
            "VideoCodec": dp_video,
            "AudioCodec": dp_audio.join(","),
        })]
    } else {
        Vec::new()
    };
    let transcoding = if ask.forced {
        Vec::new()
    } else {
        vec![json!({
            "Container": "mkv",
            "Type": "Video",
            "VideoCodec": target_video,
            "AudioCodec": if target_audio.is_empty() { "aac".to_string() } else { target_audio.join(",") },
            "Protocol": "http",
            "Context": "Streaming",
            "CopyTimestamps": true,
            "BreakOnNonKeyFrames": false,
        })]
    };
    let mut codec = Vec::new();
    if !ask.forced {
        codec.push(json!({
            "Type": "Video",
            "Conditions": [
                cond("LessThanEqual", "Width", &w.to_string()),
                cond("LessThanEqual", "Height", &h.to_string()),
                cond("LessThanEqual", "VideoBitDepth", "10"),
            ],
        }));
        for (c, ch) in &caps.audio_channels {
            if caps.audio_has(c) {
                codec.push(json!({
                    "Type": "VideoAudio",
                    "Codec": jf_audio(c),
                    "Conditions": [cond("LessThanEqual", "AudioChannels", &ch.to_string())],
                }));
            }
        }
    }
    // `SubtitleProfiles` is how the server is told which deliveries are available, and it picks
    // `Encode` (a burn, re-encoding video nobody asked to re-encode) only when nothing else fits.
    // So an EMPTY list is not "no subtitles" — it is a request to burn whichever track the item
    // defaults to. Offer the soft methods on every ask that is not an explicit burn, including the
    // transcode ask: a text track delivered as a sidecar keeps the video lane copyable.
    let mut subs = Vec::new();
    if !ask.burn {
        if ask.direct {
            // Direct play feeds the container's own bytes, so an embedded track rides along and
            // the client's renderer draws it.
            subs.extend(EMBED_SUBS.iter().map(|f| json!({ "Format": f, "Method": "Embed" })));
        } else {
            // On a converted stream only the bitmap formats need muxing in — they have no text
            // form to fetch, and `Embed` is the only delivery that is not a burn.
            subs.extend(IMAGE_SUBS.iter().map(|f| json!({ "Format": f, "Method": "Embed" })));
        }
        subs.extend(EXTERNAL_SUBS.iter().map(|f| json!({ "Format": f, "Method": "External" })));
    }
    json!({
        "Name": crate::plex::identity::PRODUCT,
        "MaxStreamingBitrate": max_streaming_bps,
        "MaxStaticBitrate": max_static_bps,
        "DirectPlayProfiles": direct_play,
        "TranscodingProfiles": transcoding,
        "ContainerProfiles": [],
        "CodecProfiles": codec,
        "SubtitleProfiles": subs,
    })
}

fn cond(condition: &str, property: &str, value: &str) -> Value {
    json!({ "Condition": condition, "Property": property, "Value": value, "IsRequired": false })
}

/// One query value of a URL, undecoded.
fn query_param<'a>(url: &'a str, key: &str) -> Option<&'a str> {
    let q = url.split_once('?')?.1;
    q.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        k.eq_ignore_ascii_case(key).then_some(v)
    })
}

/// `TranscodeReasons` of a TranscodingUrl, split (`%2C`-joined on the wire).
fn transcode_reasons(url: &str) -> Vec<String> {
    query_param(url, "TranscodeReasons")
        .map(|v| v.replace("%2C", ",").replace("%2c", ","))
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn video_reason(r: &str) -> bool {
    r.starts_with("Video")
        || matches!(r, "RefFramesNotSupported" | "AnamorphicVideoNotSupported" | "InterlacedVideoNotSupported"
            | "DirectPlayError")
}

fn audio_reason(r: &str) -> bool {
    r.starts_with("Audio") || r == "SecondaryAudioNotSupported"
}

/// The selected source stream of `kind` (`Video`/`Audio`): the explicit index, else the
/// source's default, else the first of that type.
fn pick<'a>(src: &'a MediaSourceInfo, kind: &str, index: Option<i64>) -> Option<&'a MediaStream> {
    let of_kind = || src.media_streams.iter().filter(move |s| s.kind == kind);
    let default_index = match kind {
        "Audio" => src.default_audio_stream_index,
        _ => None,
    };
    index
        .or(default_index)
        .and_then(|i| of_kind().find(|s| s.index == i))
        .or_else(|| of_kind().find(|s| s.is_default))
        .or_else(|| of_kind().next())
}

fn decision_stream(stream_type: i64, codec: &str, decision: &str, src: Option<&MediaStream>) -> Stream {
    Stream {
        id: src.map(|s| ids::track_id(s.index)).unwrap_or(0),
        stream_type,
        codec: convert::codec(codec),
        decision: decision.to_string(),
        channels: src.and_then(|s| s.channels).unwrap_or(0),
        ..Default::default()
    }
}

/// The PMS-shaped verdict for one PlaybackInfo answer.
fn verdict(rk: &str, code: i64, text: &str, part_decision: &str, streams: Vec<Stream>) -> MediaContainer {
    MediaContainer {
        general_decision_code: Some(code),
        general_decision_text: text.to_string(),
        mde_decision_code: Some(code),
        transcode_decision_code: (part_decision == "transcode").then_some(code),
        metadata: vec![Metadata {
            rating_key: rk.to_string(),
            media: vec![Media {
                part: vec![MediaPart { decision: part_decision.to_string(), stream: streams, ..Default::default() }],
                ..Default::default()
            }],
            ..Default::default()
        }],
        size: 1,
        ..Default::default()
    }
}

fn refused(rk: &str, why: &str) -> MediaContainer {
    verdict(rk, DECISION_UNPLAYABLE, why, "", Vec::new())
}

/// Output codec of one lane: the source's own when the lane is copied, else the head of the
/// target list the URL names.
fn lane_codec(url: &str, list_key: &str, src: Option<&MediaStream>, copied: bool) -> String {
    let source = src.and_then(|s| s.codec.clone()).unwrap_or_default().to_ascii_lowercase();
    let list = query_param(url, list_key).unwrap_or("").replace("%2C", ",");
    let in_list = list.split(',').any(|c| c.eq_ignore_ascii_case(&source));
    if copied && (in_list || list.is_empty() || list.eq_ignore_ascii_case("copy")) && !source.is_empty() {
        source
    } else {
        list.split(',').next().filter(|c| !c.is_empty()).unwrap_or(&source).to_ascii_lowercase()
    }
}

/// What one PlaybackInfo negotiation asks for, beside the profile.
struct InfoAsk {
    /// `EnableDirectPlay` — offer the file's own bytes.
    direct_play: bool,
    /// `EnableDirectStream` — offer a container/audio change with the video copied. Defaults to
    /// true on the server and is the documented middle rung between direct play and a re-encode;
    /// withdrawing it collapses every container mismatch into a full video transcode.
    direct_stream: bool,
    /// `AllowVideoStreamCopy`.
    allow_video_copy: bool,
    /// `StartTimeTicks` — where this playback begins, so a conversion the server has to build is
    /// primed at the resume point rather than at zero.
    start_ticks: i64,
    audio_index: Option<i64>,
    subtitle_index: Option<i64>,
    /// `AlwaysBurnInSubtitleWhenTranscoding` — overrides the subtitle profiles and forces `Encode`.
    burn_subtitle: bool,
}

impl Jf<'_> {
    fn playback_info(&self, guid: &str, profile: Value, ask: &InfoAsk) -> Option<PlaybackInfoResponse> {
        let uid = self.user_id()?;
        // `MaxAudioChannels` is a per-request bound and has no profile equivalent. Without it the
        // server has no reason to downmix, so a 5.1 or 7.1 track reaches a stereo panel at its own
        // channel count and the pipeline plays what it can of it.
        let max_channels = plx_platform::devcaps::caps()
            .audio_channels
            .values()
            .copied()
            .max()
            .filter(|&ch| ch > 0);
        let body = json!({
            "UserId": uid,
            "MaxStreamingBitrate": profile["MaxStreamingBitrate"].clone(),
            "MaxAudioChannels": max_channels,
            "StartTimeTicks": ask.start_ticks,
            "AudioStreamIndex": ask.audio_index,
            "SubtitleStreamIndex": ask.subtitle_index.unwrap_or(-1),
            "DeviceProfile": profile,
            "EnableDirectPlay": ask.direct_play,
            "EnableDirectStream": ask.direct_stream,
            "EnableTranscoding": true,
            "AllowVideoStreamCopy": ask.allow_video_copy,
            "AllowAudioStreamCopy": true,
            // Movies and episodes are static files. `RequiresOpening` is only set for Live TV,
            // IPTV and in-progress recordings, none of which this client offers, so there is no
            // live stream to open or close.
            "AutoOpenLiveStream": false,
            "AlwaysBurnInSubtitleWhenTranscoding": ask.burn_subtitle,
        });
        self.post(&format!("/Items/{guid}/PlaybackInfo"), &body)
    }

    /// The direct-play verdict (`mde_decision` / `mde_decision_forced`).
    pub fn mde_decision(&self, rk: &str, session: &str, audio_stream_id: i64, subtitle_stream_id: i64, forced: bool) -> Option<MediaContainer> {
        let guid = self.guid(rk)?;
        let audio = (audio_stream_id > 0).then(|| ids::stream_index(audio_stream_id));
        let sub = (subtitle_stream_id > 0).then(|| ids::stream_index(subtitle_stream_id));
        let caps = plx_platform::devcaps::caps();
        let profile = device_profile(caps, &ProfileAsk { direct: true, forced, ceiling: None, burn: false });
        // Direct stream is offered on the ordinary ask so a container mismatch answers as a video
        // copy; strict Original withdraws it, because that mode means the file as it is or nothing.
        let r = self.playback_info(&guid, profile, &InfoAsk {
            direct_play: true,
            direct_stream: !forced,
            allow_video_copy: true,
            start_ticks: 0,
            audio_index: audio,
            subtitle_index: sub,
            burn_subtitle: false,
        })?;
        if let Some(e) = r.error_code.as_deref().filter(|e| !e.is_empty()) {
            return Some(refused(rk, e));
        }
        let Some(src) = r.media_sources.first() else {
            return Some(refused(rk, "The server offered no media source."));
        };
        let url = src.transcoding_url.clone().unwrap_or_default();
        let direct = src.supports_direct_play;
        let reasons = transcode_reasons(&url);
        let v = pick(src, "Video", None);
        let a = pick(src, "Audio", audio);
        let video_copy = direct || !reasons.iter().any(|r| video_reason(r));
        let audio_copy = direct || !reasons.iter().any(|r| audio_reason(r));
        put_session(session, Session {
            item_guid: guid,
            media_source_id: src.id.clone(),
            play_session_id: r.play_session_id.clone().unwrap_or_default(),
            play_method: play_method(direct, video_copy),
            transcoding_url: url,
            audio_index: audio.or(a.map(|s| s.index)),
            subtitle_index: sub,
            started: false,
            start_time_ticks: 0,
        });
        // `SupportsDirectStream` is the middle rung and counts as playable: the server will copy
        // the video even where it refuses the file as-is. Only when all three are refused is there
        // nothing left to play.
        if !direct && !src.supports_transcoding && !src.supports_direct_stream {
            return Some(refused(rk, "Neither direct play nor conversion is available."));
        }
        let vcodec = v.and_then(|s| s.codec.clone()).unwrap_or_default();
        let acodec = a.and_then(|s| s.codec.clone()).unwrap_or_default();
        Some(verdict(rk, DECISION_OK, if direct { "Direct play OK." } else { "Direct play not available; Conversion OK." },
            if direct { "directplay" } else { "transcode" },
            vec![
                decision_stream(1, &vcodec, if video_copy { "copy" } else { "transcode" }, v),
                decision_stream(2, &acodec, if audio_copy { "copy" } else { "transcode" }, a),
            ]))
    }

    /// Register a transcode: PlaybackInfo with direct play withdrawn, the TranscodingUrl kept for
    /// [`Self::transcode_start_url`], and the OUTPUT codecs reported per lane.
    pub fn transcode_decision(&self, spec: &TranscodeSpec) -> Option<MediaContainer> {
        let guid = self.guid(spec.rating_key)?;
        let c = spec.contract;
        let allow_video_copy = c.remux || !c.no_video_copy;
        let ceiling = if c.remux { None } else { Some(c.ceiling.unwrap_or(Ceiling::NATIVE_4K)) };
        let audio = (spec.audio_stream_id > 0).then(|| ids::stream_index(spec.audio_stream_id));
        let sub = (spec.subtitle_stream_id > 0).then(|| ids::stream_index(spec.subtitle_stream_id));
        let start_ticks = match spec.offset {
            TranscodeOffset::Fresh => 0,
            TranscodeOffset::AtMicros(us) => us.saturating_mul(10),
        };
        // A positive `subtitle_stream_id` on the spec IS the request to burn (see `TranscodeSpec`),
        // so the profile withdraws its soft methods and the request states the override. Both
        // halves read the same flag: offering `External` while also forcing `Encode` would ask the
        // server for two different things.
        let burn = sub.is_some();
        let profile = device_profile(plx_platform::devcaps::caps(),
            &ProfileAsk { direct: false, forced: false, ceiling, burn });
        let r = self.playback_info(&guid, profile, &InfoAsk {
            direct_play: false,
            // The route asked for a conversion, and a copied video lane is the cheapest one that
            // satisfies it. Withdrawing this would force a re-encode even for a bare remux.
            direct_stream: true,
            allow_video_copy,
            start_ticks,
            audio_index: audio,
            subtitle_index: sub,
            burn_subtitle: burn,
        })?;
        if let Some(e) = r.error_code.as_deref().filter(|e| !e.is_empty()) {
            return Some(refused(spec.rating_key, e));
        }
        let Some(src) = r.media_sources.first() else {
            return Some(refused(spec.rating_key, "The server offered no media source."));
        };
        let Some(url) = src.transcoding_url.clone().filter(|u| !u.is_empty()) else {
            return Some(refused(spec.rating_key, "Neither direct play nor conversion is available."));
        };
        let reasons = transcode_reasons(&url);
        let v = pick(src, "Video", None);
        let a = pick(src, "Audio", audio);
        let video_copy = allow_video_copy && !reasons.iter().any(|r| video_reason(r));
        let audio_copy = !reasons.iter().any(|r| audio_reason(r));
        let vcodec = lane_codec(&url, "VideoCodec", v, video_copy);
        let acodec = lane_codec(&url, "AudioCodec", a, audio_copy);
        put_session(spec.session, Session {
            item_guid: guid,
            media_source_id: src.id.clone(),
            play_session_id: r.play_session_id.clone().unwrap_or_default(),
            // A copied video lane is a `DirectStream`, however it was reached: the route asked for
            // a conversion, but what the server agreed to produce is what the session must report,
            // or the dashboard shows a re-encode that is not running.
            play_method: play_method(false, video_copy),
            transcoding_url: url,
            audio_index: audio.or(a.map(|s| s.index)),
            subtitle_index: sub,
            started: false,
            start_time_ticks: 0,
        });
        Some(verdict(spec.rating_key, DECISION_OK, "Direct play not available; Conversion OK.", "transcode", vec![
            decision_stream(1, &vcodec, if video_copy { "copy" } else { "transcode" }, v),
            decision_stream(2, &acodec, if audio_copy { "copy" } else { "transcode" }, a),
        ]))
    }

    /// The registered TranscodingUrl, which already carries its `ApiKey` (and so is never
    /// re-tokened). A restart offset the URL does not encode is appended as `StartTimeTicks`.
    pub fn transcode_start_url(&self, spec: &TranscodeSpec) -> StreamUrl {
        let mut path = session(spec.session).map(|s| s.transcoding_url).unwrap_or_default();
        if let TranscodeOffset::AtMicros(us) = spec.offset {
            if !path.is_empty() && query_param(&path, "StartTimeTicks").is_none() {
                path.push_str(&format!("&StartTimeTicks={}", us.saturating_mul(10)));
            }
        }
        let path = if path.is_empty() || super::url::has_api_key(&path) {
            path
        } else {
            super::url::with_api_key(&path, &self.token())
        };
        StreamUrl { origin: self.origin().clone(), path }
    }

    /// The part URL for direct play: the static stream plus this playback's PlaySessionId.
    pub fn direct_play_url(&self, part_key: &str, session_key: &str) -> StreamUrl {
        let mut path = part_key.to_string();
        if let Some(s) = session(session_key).filter(|s| !s.play_session_id.is_empty()) {
            let sep = if path.contains('?') { '&' } else { '?' };
            path = format!("{path}{sep}PlaySessionId={}", crate::plex::urlenc_str(&s.play_session_id));
        }
        StreamUrl { origin: self.origin().clone(), path: super::url::with_api_key(&path, &self.token()) }
    }

    /// End the server's encoder for `session`: Jellyfin 12 has no `/Videos/ActiveEncodings` route,
    /// and reporting the PlaySessionId stopped is what kills its ffmpeg job.
    ///
    /// Every method except `DirectPlay` has such a job — a `DirectStream` remux is still ffmpeg
    /// copying streams into a new container, and gating this on the literal `Transcode` would
    /// leave one running for the server's whole idle timeout.
    pub fn transcode_stop(&self, session_key: &str) -> bool {
        let Some(s) = session(session_key) else { return true };
        if s.play_method == DIRECT_PLAY || s.play_session_id.is_empty() {
            return true;
        }
        let ok = self.report("/Sessions/Playing/Stopped", &s, 0, false, None);
        update_session(session_key, |s| s.started = false);
        ok
    }

    pub fn timeline(&self, r: &TimelineReport) -> bool {
        let mut s = session(r.session).unwrap_or_else(|| Session {
            item_guid: self.guid(r.rating_key).unwrap_or_default(),
            play_method: DIRECT_PLAY,
            ..Default::default()
        });
        if s.item_guid.is_empty() {
            return false;
        }
        if r.audio_stream_id > 0 {
            s.audio_index = Some(ids::stream_index(r.audio_stream_id));
        }
        if r.subtitle_stream_id > 0 {
            s.subtitle_index = Some(ids::stream_index(r.subtitle_stream_id));
        }
        // Stamped once, on the first report, and carried by every later one. Taken here rather
        // than at the decision because a decision that never reaches the engine (a refusal, a
        // superseded resolve) never starts a playback to attribute time to.
        if s.start_time_ticks == 0 {
            s.start_time_ticks = ticks::now_utc();
        }
        // The queue position, when the caller has one. Sent as `PlaylistItemId` so the server can
        // place this playback in the queue it was told about; "0" is this protocol's "no position"
        // and must not be forwarded as one.
        let playlist_item_id = Some(r.play_queue_item_id)
            .filter(|id| !id.is_empty() && *id != "0")
            .map(str::to_string);
        let paused = r.state == TimelineState::Paused;
        match r.state {
            TimelineState::Stopped => {
                drop_session(r.session);
                self.report("/Sessions/Playing/Stopped", &s, r.time_ms, paused, playlist_item_id)
            }
            _ if !s.started => {
                let ok = self.report("/Sessions/Playing", &s, r.time_ms, paused, playlist_item_id);
                s.started = ok;
                put_session(r.session, s);
                ok
            }
            _ => {
                let ok = self.report("/Sessions/Playing/Progress", &s, r.time_ms, paused, playlist_item_id);
                // The start stamp is this session's and outlives the local copy above.
                put_session(r.session, s);
                ok
            }
        }
    }

    fn report(
        &self,
        path: &str,
        s: &Session,
        time_ms: i64,
        paused: bool,
        playlist_item_id: Option<String>,
    ) -> bool {
        let body = PlaybackReport {
            item_id: s.item_guid.clone(),
            media_source_id: s.media_source_id.clone(),
            play_session_id: s.play_session_id.clone(),
            position_ticks: ticks::from_ms(time_ms),
            is_paused: paused,
            can_seek: true,
            audio_stream_index: s.audio_index,
            subtitle_stream_index: s.subtitle_index,
            play_method: if s.play_method.is_empty() { DIRECT_PLAY.into() } else { s.play_method.into() },
            playback_start_time_ticks: (s.start_time_ticks > 0).then_some(s.start_time_ticks),
            playback_order: Some("Default"),
            repeat_mode: Some("RepeatNone"),
            playlist_item_id,
            failed: None,
        };
        self.post_ok(path, Some(&body))
    }

    /// Jellyfin selects tracks per request (the PlaybackInfo indexes), so there is nothing to PUT.
    pub fn select_streams(&self, _sel: &StreamSelection) -> i32 {
        200
    }

    /// The queue a play starts: the item, and — for a continuous episode — the series' episodes
    /// from it onward, as PMS's `continuous=1` window.
    pub fn create_play_queue(&self, rk: &str, continuous: bool) -> Option<MediaContainer> {
        let guid = self.guid(rk)?;
        let uid = self.user_id()?;
        let it: BaseItemDto = self.get(&super::api::Q::new(format!("/Items/{guid}")).s("userId", &uid)
            .s("Fields", "MediaSources").build())?;
        let mut rows = vec![convert::item(&it, 0)];
        if continuous && it.kind == "Episode" {
            if let Some(series) = it.series_id.as_deref().map(ids::normalize).filter(|s| !s.is_empty()) {
                let r: Option<QueryResult<BaseItemDto>> = self.get(&super::api::Q::new(format!("/Shows/{series}/Episodes"))
                    .s("userId", &uid).s("StartItemId", &guid).s("Fields", "MediaSources").i("Limit", 50).build());
                if let Some(r) = r.filter(|r| r.items.first().is_some_and(|e| ids::normalize(&e.id) == guid)) {
                    rows = r.items.iter().map(|e| convert::item(e, 0)).collect();
                }
            }
        }
        for (i, m) in rows.iter_mut().enumerate() {
            m.play_queue_item_id = i as i64 + 1;
        }
        let n = rows.len() as i64;
        Some(MediaContainer {
            play_queue_id: ids::intern(&guid),
            play_queue_selected_item_id: 1,
            play_queue_total_count: n,
            play_queue_selected_item_offset: 0,
            size: n,
            metadata: rows,
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps() -> plx_platform::devcaps::Caps {
        let mut c = plx_platform::devcaps::Caps::assumed();
        c.audio = "aac,ac3,eac3,dts".into();
        c.audio_channels.insert("dts".into(), 6);
        c
    }

    #[test]
    fn the_direct_profile_advertises_the_panels_own_decode_limits() {
        let p = device_profile(&caps(), &ProfileAsk { direct: true, forced: false, ceiling: None, burn: false });
        let dp = &p["DirectPlayProfiles"][0];
        assert_eq!(dp["VideoCodec"], "h264,hevc");
        assert_eq!(dp["AudioCodec"], "aac,ac3,eac3,dts");
        assert_eq!(p["TranscodingProfiles"][0]["Container"], "mkv");
        assert_eq!(p["TranscodingProfiles"][0]["VideoCodec"], "hevc,h264");
        assert_eq!(p["TranscodingProfiles"][0]["AudioCodec"], "ac3,eac3,aac,dts");
        let video = &p["CodecProfiles"][0]["Conditions"];
        assert_eq!(video[0]["Value"], "3840");
        assert_eq!(video[1]["Value"], "2176");
        let dts = p["CodecProfiles"].as_array().unwrap().iter().find(|c| c["Codec"] == "dts").unwrap();
        assert_eq!(dts["Conditions"][0]["Value"], "6");
        assert!(p["SubtitleProfiles"].as_array().unwrap().iter().any(|s| s["Format"] == "pgssub" && s["Method"] == "Embed"));
    }

    #[test]
    fn a_ceiling_bounds_the_original_file_as_well_as_the_encode() {
        let c = Ceiling { max_kbps: 4000, max_w: 1280, max_h: 720 };
        let p = device_profile(&caps(), &ProfileAsk { direct: false, forced: false, ceiling: Some(c), burn: true });
        assert_eq!(p["MaxStreamingBitrate"], 4_000_000);
        // The whole point: an explicit ask the direct-play branch cannot ignore.
        assert_eq!(p["MaxStaticBitrate"], 4_000_000);
        assert_eq!(p["CodecProfiles"][0]["Conditions"][0]["Value"], "1280");
        assert!(p["DirectPlayProfiles"].as_array().unwrap().is_empty());
        assert!(p["SubtitleProfiles"].as_array().unwrap().is_empty(), "a burn withdraws every soft method");
    }

    #[test]
    fn no_ceiling_leaves_both_rates_unbounded() {
        let p = device_profile(&caps(), &ProfileAsk { direct: true, forced: false, ceiling: None, burn: false });
        assert_eq!(p["MaxStreamingBitrate"], UNBOUNDED_BPS);
        assert_eq!(p["MaxStaticBitrate"], UNBOUNDED_BPS);
    }

    #[test]
    fn a_conversion_still_offers_the_soft_subtitle_methods() {
        let p = device_profile(&caps(), &ProfileAsk { direct: false, forced: false, ceiling: None, burn: false });
        let subs = p["SubtitleProfiles"].as_array().unwrap();
        assert!(!subs.is_empty(), "an empty list asks the server to burn the item's default track");
        // Text travels as a sidecar, so the video lane stays copyable...
        assert!(subs.iter().any(|s| s["Format"] == "srt" && s["Method"] == "External"));
        // ...and a bitmap track, which has no text form, is muxed rather than burned.
        assert!(subs.iter().any(|s| s["Format"] == "pgssub" && s["Method"] == "Embed"));
        // `External` is not available for a bitmap format.
        assert!(!subs.iter().any(|s| s["Format"] == "pgssub" && s["Method"] == "External"));
    }

    #[test]
    fn the_play_method_is_one_of_the_three_the_api_defines() {
        assert_eq!(play_method(true, true), "DirectPlay");
        assert_eq!(play_method(true, false), "DirectPlay");
        // A copied video lane is a remux or an audio-only conversion. The wire enum has one value
        // for both, and it is not `Transcode`.
        assert_eq!(play_method(false, true), "DirectStream");
        assert_eq!(play_method(false, false), "Transcode");
    }

    #[test]
    fn forced_original_offers_no_target_and_no_limits() {
        let p = device_profile(&caps(), &ProfileAsk { direct: true, forced: true, ceiling: None, burn: false });
        assert!(p["TranscodingProfiles"].as_array().unwrap().is_empty());
        assert!(p["CodecProfiles"].as_array().unwrap().is_empty());
    }

    #[test]
    fn transcode_reasons_split_into_lanes() {
        let url = "/videos/x/stream.mkv?VideoCodec=hevc,h264&AudioCodec=ac3&TranscodeReasons=ContainerNotSupported%2CAudioCodecNotSupported";
        let r = transcode_reasons(url);
        assert_eq!(r, ["ContainerNotSupported", "AudioCodecNotSupported"]);
        assert!(!r.iter().any(|r| video_reason(r)));
        assert!(r.iter().any(|r| audio_reason(r)));
    }

    #[test]
    fn a_copied_lane_reports_the_source_codec_and_an_encoded_one_the_target() {
        let url = "/videos/x/stream.mkv?VideoCodec=hevc,h264&AudioCodec=ac3,eac3";
        let v = MediaStream { kind: "Video".into(), codec: Some("h264".into()), ..Default::default() };
        let a = MediaStream { kind: "Audio".into(), codec: Some("truehd".into()), ..Default::default() };
        assert_eq!(lane_codec(url, "VideoCodec", Some(&v), true), "h264");
        assert_eq!(lane_codec(url, "VideoCodec", Some(&v), false), "hevc");
        assert_eq!(lane_codec(url, "AudioCodec", Some(&a), true), "ac3", "a codec off the list cannot be copied");
    }

    #[test]
    fn the_verdict_is_the_shape_the_shared_route_layer_grades() {
        let mc = verdict("7", DECISION_OK, "ok", "directplay", vec![
            decision_stream(1, "hevc", "copy", None),
            decision_stream(2, "dts", "copy", None),
        ]);
        let part = mc.metadata[0].first_part().unwrap();
        assert_eq!(part.decision, "directplay");
        assert_eq!(part.stream[1].codec, "dca", "Plex's spelling, which the payload builder expects");
        assert_eq!(mc.general_decision_code, Some(1000));
        assert_eq!(refused("7", "x").general_decision_code, Some(2000));
    }
}
