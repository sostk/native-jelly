//! Playback against Jellyfin, answered in the shape the route layer already reads.
//!
//! PMS adjudicates a play with `/video/:/transcode/universal/decision`; Jellyfin with
//! `POST /Items/{id}/PlaybackInfo` and a DeviceProfile. Each decision here asks PlaybackInfo and
//! synthesizes the PMS verdict container the route grades — `generalDecisionCode`,
//! `Part.decision` (`directplay`/`transcode`), and the per-stream `decision`/`codec` pair whose
//! codecs are the transcode OUTPUT (`route::plan::decision_codecs`).
//!
//! What PMS keeps server-side under a session id (the registered decision, the selected tracks)
//! Jellyfin hands back as a `PlaySessionId` and a `TranscodingUrl`, so this module keeps them in a
//! table keyed by the app's own session string: the start URL, the progress reports and the stop
//! all read the row the decision wrote.
use super::models::*;
use super::{api::Jf, convert, ids, ticks};
use crate::plex::{
    Ceiling, MediaContainer, Media, MediaPart, Metadata, Stream, StreamSelection, StreamUrl,
    TimelineReport, TimelineState, TranscodeOffset, TranscodeSpec,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// No bound in practice — what direct play and a remux advertise (PMS's `NATIVE_4K` rate is a
/// cap only on the re-encode branch, and so is this).
const UNBOUNDED_BPS: i64 = 200_000_000;
/// PMS's "Direct play OK." / "Neither direct play nor conversion is available." codes.
const DECISION_OK: i64 = 1000;
const DECISION_UNPLAYABLE: i64 = 2000;

#[derive(Debug, Clone, Default)]
struct Session {
    item_guid: String,
    media_source_id: String,
    play_session_id: String,
    /// `DirectPlay | Transcode`.
    play_method: &'static str,
    transcoding_url: String,
    audio_index: Option<i64>,
    subtitle_index: Option<i64>,
    /// `/Sessions/Playing` has been sent; later reports are `/Progress`.
    started: bool,
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

/// Subtitle formats Original renders itself, in Jellyfin's spelling (`pgssub`, `dvdsub`, …).
const EMBED_SUBS: [&str; 10] =
    ["srt", "subrip", "ass", "ssa", "pgssub", "dvdsub", "dvbsub", "mov_text", "webvtt", "vtt"];
const EXTERNAL_SUBS: [&str; 5] = ["srt", "subrip", "ass", "ssa", "vtt"];

pub(crate) struct ProfileAsk {
    /// Offer direct play (and the Embed/External subtitle methods that go with it).
    pub direct: bool,
    /// Strict Original: the software feed's formats, no device limits, no transcode target.
    pub forced: bool,
    /// The re-encode bound; `None` advertises the panel's own.
    pub ceiling: Option<Ceiling>,
    /// Burn the selected subtitle (no Embed/External profile on the transcode).
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
    let mut max_bps = UNBOUNDED_BPS;
    if let Some(c) = ask.ceiling {
        w = w.min(c.max_w as u32);
        h = h.min(c.max_h as u32);
        max_bps = c.max_kbps.saturating_mul(1000);
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
    let mut subs = Vec::new();
    if ask.direct && !ask.burn {
        subs.extend(EMBED_SUBS.iter().map(|f| json!({ "Format": f, "Method": "Embed" })));
        subs.extend(EXTERNAL_SUBS.iter().map(|f| json!({ "Format": f, "Method": "External" })));
    }
    json!({
        "Name": crate::plex::identity::PRODUCT,
        "MaxStreamingBitrate": max_bps,
        "MaxStaticBitrate": UNBOUNDED_BPS,
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

impl Jf<'_> {
    fn playback_info(
        &self,
        guid: &str,
        profile: Value,
        enable_direct: bool,
        allow_video_copy: bool,
        start_ticks: i64,
        audio_index: Option<i64>,
        subtitle_index: Option<i64>,
    ) -> Option<PlaybackInfoResponse> {
        let uid = self.user_id()?;
        let body = json!({
            "UserId": uid,
            "MaxStreamingBitrate": profile["MaxStreamingBitrate"].clone(),
            "StartTimeTicks": start_ticks,
            "AudioStreamIndex": audio_index,
            "SubtitleStreamIndex": subtitle_index.unwrap_or(-1),
            "DeviceProfile": profile,
            "EnableDirectPlay": enable_direct,
            "EnableDirectStream": false,
            "EnableTranscoding": true,
            "AllowVideoStreamCopy": allow_video_copy,
            "AllowAudioStreamCopy": true,
            "AutoOpenLiveStream": false,
            "AlwaysBurnInSubtitleWhenTranscoding": subtitle_index.is_some_and(|i| i >= 0),
        });
        self.post(&format!("/Items/{guid}/PlaybackInfo"), &body)
    }

    /// The direct-play verdict (`mde_decision` / `mde_decision_forced`).
    pub fn mde_decision(&self, rk: &str, session: &str, audio_stream_id: i64, subtitle_stream_id: i64, forced: bool) -> Option<MediaContainer> {
        let guid = ids::guid_of_key(rk)?;
        let audio = (audio_stream_id > 0).then(|| ids::stream_index(audio_stream_id));
        let sub = (subtitle_stream_id > 0).then(|| ids::stream_index(subtitle_stream_id));
        let caps = plx_platform::devcaps::caps();
        let profile = device_profile(caps, &ProfileAsk { direct: true, forced, ceiling: None, burn: false });
        let r = self.playback_info(&guid, profile, true, true, 0, audio, sub)?;
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
            play_method: if direct { "DirectPlay" } else { "Transcode" },
            transcoding_url: url,
            audio_index: audio.or(a.map(|s| s.index)),
            subtitle_index: sub,
            started: false,
        });
        if !direct && !src.supports_transcoding {
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
        let guid = ids::guid_of_key(spec.rating_key)?;
        let c = spec.contract;
        let allow_video_copy = c.remux || !c.no_video_copy;
        let ceiling = if c.remux { None } else { Some(c.ceiling.unwrap_or(Ceiling::NATIVE_4K)) };
        let audio = (spec.audio_stream_id > 0).then(|| ids::stream_index(spec.audio_stream_id));
        let sub = (spec.subtitle_stream_id > 0).then(|| ids::stream_index(spec.subtitle_stream_id));
        let start_ticks = match spec.offset {
            TranscodeOffset::Fresh => 0,
            TranscodeOffset::AtMicros(us) => us.saturating_mul(10),
        };
        let profile = device_profile(plx_platform::devcaps::caps(),
            &ProfileAsk { direct: false, forced: false, ceiling, burn: sub.is_some() });
        let r = self.playback_info(&guid, profile, false, allow_video_copy, start_ticks, audio, sub)?;
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
            play_method: "Transcode",
            transcoding_url: url,
            audio_index: audio.or(a.map(|s| s.index)),
            subtitle_index: sub,
            started: false,
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

    /// End the server's transcode for `session`: Jellyfin 12 has no ActiveEncodings route, and
    /// reporting the PlaySessionId stopped is what kills its ffmpeg job.
    pub fn transcode_stop(&self, session_key: &str) -> bool {
        let Some(s) = session(session_key) else { return true };
        if s.play_method != "Transcode" || s.play_session_id.is_empty() {
            return true;
        }
        let ok = self.report("/Sessions/Playing/Stopped", &s, 0, false);
        update_session(session_key, |s| s.started = false);
        ok
    }

    pub fn timeline(&self, r: &TimelineReport) -> bool {
        let mut s = session(r.session).unwrap_or_else(|| Session {
            item_guid: ids::guid_of_key(r.rating_key).unwrap_or_default(),
            play_method: "DirectPlay",
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
        let paused = r.state == TimelineState::Paused;
        match r.state {
            TimelineState::Stopped => {
                drop_session(r.session);
                self.report("/Sessions/Playing/Stopped", &s, r.time_ms, paused)
            }
            _ if !s.started => {
                let ok = self.report("/Sessions/Playing", &s, r.time_ms, paused);
                s.started = ok;
                put_session(r.session, s);
                ok
            }
            _ => self.report("/Sessions/Playing/Progress", &s, r.time_ms, paused),
        }
    }

    fn report(&self, path: &str, s: &Session, time_ms: i64, paused: bool) -> bool {
        let body = PlaybackReport {
            item_id: s.item_guid.clone(),
            media_source_id: s.media_source_id.clone(),
            play_session_id: s.play_session_id.clone(),
            position_ticks: ticks::from_ms(time_ms),
            is_paused: paused,
            can_seek: true,
            audio_stream_index: s.audio_index,
            subtitle_stream_index: s.subtitle_index,
            play_method: if s.play_method.is_empty() { "DirectPlay".into() } else { s.play_method.into() },
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
        let guid = ids::guid_of_key(rk)?;
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
    fn the_direct_profile_advertises_what_the_pms_profile_does() {
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
    fn a_ceiling_bounds_the_encode_and_a_burn_drops_the_soft_methods() {
        let c = Ceiling { max_kbps: 4000, max_w: 1280, max_h: 720 };
        let p = device_profile(&caps(), &ProfileAsk { direct: false, forced: false, ceiling: Some(c), burn: true });
        assert_eq!(p["MaxStreamingBitrate"], 4_000_000);
        assert_eq!(p["CodecProfiles"][0]["Conditions"][0]["Value"], "1280");
        assert!(p["DirectPlayProfiles"].as_array().unwrap().is_empty());
        assert!(p["SubtitleProfiles"].as_array().unwrap().is_empty());
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
    fn the_verdict_is_the_shape_the_route_grades() {
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
