//! What this client can play, as one answer: the television's decoders, as the platform measured
//! them, intersected with what the buffer-feed pipeline can demux, feed and render.
//!
//! Three layers, bottom up, and each claim the server receives traces through them:
//!
//! * **Platform detection** — `nj_platform::devcaps` reads the device's codec table at boot, and
//!   the webOS port publishes the Dolby Vision and HDR10 probes (`devcaps::dv`, `devcaps::hdr`).
//!   Nothing above names webOS.
//! * **Client capabilities** — [`Capabilities`]: those measurements combined with the pipeline's
//!   own limits (the constants below), plus an explicit [`Support::Unknown`] for what neither can
//!   tell.
//! * **Server vocabulary** — `jf::playback::device_profile` turns a [`Capabilities`] into a
//!   Jellyfin `DeviceProfile` and reads nothing else, so a profile can be built from any
//!   capability set in a host test.
//!
//! What a playback may *ask* for on top of this (the user's Direct Play mode and quality ceiling,
//! the link policy) is policy, not capability, and stays with the route.
use nj_platform::devcaps::{self, dv::DvCapability, hdr::HdrCapability, Caps};

/// An answer that admits it was not measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Support {
    Yes,
    No,
    Unknown,
}

impl Support {
    fn label(self) -> &'static str {
        match self {
            Self::Yes => "yes",
            Self::No => "no",
            Self::Unknown => "unknown",
        }
    }
}

/// Containers the demuxer (bundled libavformat over the app's own reader) is fed as whole files,
/// in Jellyfin's spelling. WebM is Matroska to the demuxer (`--enable-demuxer=matroska` reads
/// both), and it is the container VP9 and AV1 files most often come in.
pub(crate) const CONTAINERS: [&str; 5] = ["mkv", "webm", "mp4", "m4v", "mov"];

/// Video codecs the feed hands to the platform decoder (`ff::VideoFraming` frames each). VP9 and
/// AV1 are fed only where the device table lists their decoder (`devcaps::Caps::{vp9, av1}`), so
/// they are offered only there (`route::plan::video_feed_supported`).
const PIPELINE_VIDEO: [&str; 4] = ["h264", "hevc", "vp9", "av1"];

/// Subtitle formats the client draws itself out of the container, in Jellyfin's spelling.
pub(crate) const EMBEDDED_SUBTITLES: [&str; 10] =
    ["srt", "subrip", "ass", "ssa", "pgssub", "dvdsub", "dvbsub", "mov_text", "webvtt", "vtt"];
/// Text formats the client fetches as a sidecar file.
pub(crate) const SIDECAR_SUBTITLES: [&str; 5] = ["srt", "subrip", "ass", "ssa", "vtt"];
/// Bitmap formats: no text form, so the only soft delivery is in the container.
pub(crate) const BITMAP_SUBTITLES: [&str; 3] = ["pgssub", "dvdsub", "dvbsub"];

/// Subtitle codecs the renderer draws (`ff.rs` / the track menu), in the app's spelling plus the
/// aliases they arrive as (`movtext`; `dvd` beside `vobsub` / `dvd_subtitle`). Obscure `ff.rs`
/// Plain aliases (`vplayer`, `jacosub`, …) stay off on purpose. Every [`EMBEDDED_SUBTITLES`] entry
/// is in this set once spelled the app's way (pinned by a test): `jf::convert::codec` names WebVTT
/// `vtt`, which `ff.rs` draws as plain text.
pub const RENDERED_SUBTITLES: &str = "srt,subrip,ass,ssa,mov_text,movtext,webvtt,vtt,text,pgs,hdmv_pgs_subtitle,vobsub,dvd,dvd_subtitle,dvdsub,dvb_subtitle,dvbsub";

/// The client's capabilities: one device measurement read through the pipeline's limits.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Capabilities<'a> {
    device: &'a Caps,
    dolby_vision: DvCapability,
    hdr: HdrCapability,
}

/// The capability value the NEXT play start would derive: the write-once boot snapshot less the
/// install's stored native audio refusals in force on this build (`devcaps::effective_caps`). A
/// play captures it once (`route::ResolveEnv::caps`) and every gate and profile of that attempt
/// reads the captured value; this reader is for what a page shows before Play is pressed.
pub(crate) fn start_caps() -> std::sync::Arc<Caps> {
    devcaps::effective_caps(&crate::catalog::session::peek().applied_refusals())
}

impl<'a> Capabilities<'a> {
    /// One attempt's measured capabilities: `device` is the attempt's own snapshot, the Dolby
    /// Vision and HDR10 answers are the platform's.
    pub(crate) fn for_attempt(device: &'a Caps) -> Self {
        Self::of(device, devcaps::dv::capability()).with_hdr(devcaps::hdr::capability())
    }
}

impl<'a> Capabilities<'a> {
    /// A capability set whose HDR10 support is not measured.
    pub(crate) fn of(device: &'a Caps, dolby_vision: DvCapability) -> Self {
        Self { device, dolby_vision, hdr: HdrCapability::Unknown }
    }

    pub(crate) fn with_hdr(self, hdr: HdrCapability) -> Self {
        Self { hdr, ..self }
    }

    /// Video codecs direct play can use: the pipeline's, where the SoC decodes them. The same rule
    /// as the local gate (`route::plan::video_direct_plays`) and the Load refusal
    /// (`player::engine::load_video_refused`), which `route::plan_codec_agreement_tests` holds
    /// together over every capability set.
    pub(crate) fn video_codecs(&self) -> Vec<&'static str> {
        PIPELINE_VIDEO
            .into_iter()
            .filter(|c| match *c {
                "h264" => true,
                "hevc" => self.device.hevc,
                "vp9" => self.device.vp9,
                "av1" => self.device.av1,
                _ => false,
            })
            .collect()
    }

    /// The pipeline's video codecs past the device's bounds — Strict Original's ask. H.264 and HEVC
    /// are offered whatever the table says (Strict Original takes the file as it is), but VP9 and
    /// AV1 still need their rows: a set with no such decoder cannot play the file at all, and the
    /// Load would refuse it (`route::plan::video_feed_supported`).
    pub(crate) fn pipeline_video_codecs(&self) -> Vec<&'static str> {
        PIPELINE_VIDEO
            .into_iter()
            .filter(|c| match *c {
                "vp9" => self.device.vp9,
                "av1" => self.device.av1,
                _ => true,
            })
            .collect()
    }

    /// Per-codec bounds beyond the shared raster: `(codec, max width, max height, max bit depth)`
    /// for each table-gated codec this set decodes. The width and height are the table's own row
    /// (0 = not stated, so only the shared bound applies); VP9's bit depth is
    /// `devcaps::Caps::vp9_bit_depth`, which is 10 only where 10-bit VP9 was measured.
    pub(crate) fn codec_row_bounds(&self) -> Vec<(&'static str, u32, u32, Option<u32>)> {
        let mut rows = Vec::new();
        if self.device.vp9 {
            let (w, h, _) = self.device.vp9_row;
            rows.push(("vp9", w, h, Some(self.device.vp9_bit_depth)));
        }
        if self.device.av1 {
            let (w, h, _) = self.device.av1_row;
            rows.push(("av1", w, h, None));
        }
        rows
    }

    /// Audio codecs the pipeline feeds AND this television decodes, in Jellyfin's spelling.
    pub(crate) fn audio_codecs(&self) -> Vec<&'a str> {
        self.device.audio.split(',').filter(|c| !c.is_empty()).map(jf_codec).collect()
    }

    /// The pipeline's audio codecs past the device's channel ceilings — Strict Original's ask. DTS
    /// still needs its table row (`devcaps::Caps::forced_feeds_audio`): a set with no DTS decoder
    /// takes the Load and never starts.
    pub(crate) fn pipeline_audio_codecs(&self) -> Vec<&'static str> {
        devcaps::DP_AUDIO_CODECS.split(',').filter(|c| self.device.forced_feeds_audio(c)).map(jf_codec).collect()
    }

    /// Channel ceilings per decodable audio codec, in Jellyfin's spelling: the measured table row
    /// and any firmware or learned hard limit (`Caps::native_audio_limits`), the lower winning —
    /// the same two bounds the local gate (`Caps::audio_supports`) applies, so a limit with no
    /// table row still reaches the server.
    pub(crate) fn audio_channel_limits(&self) -> Vec<(&'a str, u32)> {
        let device: &'a Caps = self.device;
        let mut limits: std::collections::BTreeMap<&'a str, u32> = std::collections::BTreeMap::new();
        for (codec, max) in device.audio_channels.iter().chain(device.native_audio_limits.iter()) {
            if *max > 0 && device.audio_has(codec) {
                let slot = limits.entry(codec.as_str()).or_insert(*max);
                *slot = (*slot).min(*max);
            }
        }
        limits.into_iter().map(|(c, ch)| (jf_codec(c), ch)).collect()
    }

    /// Firmware exclusions retained by Strict Original, independently of the
    /// ordinary device ceilings that Forced mode intentionally omits.
    pub(crate) fn forced_audio_channel_limits(&self) -> impl Iterator<Item = (&str, u32)> {
        self.device.native_audio_limits.iter()
            .filter(|(codec, max)| **max > 0 && self.device.forced_feeds_audio(codec))
            .map(|(codec, max)| (jf_codec(codec), *max))
    }

    /// The highest measured channel count of any audio codec, `None` when none was measured.
    pub(crate) fn max_audio_channels(&self) -> Option<u32> {
        self.device.audio_channels.values().copied().max().filter(|&ch| ch > 0)
    }

    /// The decoder raster bound, applied to every codec at once (`devcaps::Caps::hevc_max`).
    pub(crate) fn max_resolution(&self) -> (u32, u32) {
        self.device.hevc_max
    }

    /// No device bitrate bound is measured (the table's bitrate column is not read), so a
    /// playback's bound is the user's quality ceiling alone.
    pub(crate) fn max_bitrate_bps(&self) -> Option<i64> {
        None
    }

    /// The codecs a re-encode may target (`devcaps::Caps::encode_vcodec`), H.264 first. Jellyfin
    /// encodes to the head of the list after moving HEVC and AV1 to the end unless its administrator
    /// allowed encoding them (`EncodingHelper.ShiftVideoCodecsIfNeeded`), a setting a client cannot
    /// read; H.264 is never moved, so a head of H.264 is the codec that arrives. The others stay
    /// listed so a source in them can still be copied. The web client orders its list the same way.
    ///
    /// VP9 and AV1 trail the list where the table lists their decoders, so a progressive
    /// conversion that only has to change the audio or the container can COPY such a source
    /// (Jellyfin copies only a codec the target list names). They are never the head, so they are
    /// never what the server encodes to.
    pub(crate) fn transcode_video_codecs(&self) -> Vec<&'static str> {
        let mut codecs = match self.device.encode_vcodec() {
            "h264" => vec!["h264"],
            other => vec!["h264", other],
        };
        if self.device.vp9 {
            codecs.push("vp9");
        }
        if self.device.av1 {
            codecs.push("av1");
        }
        codecs
    }

    /// Audio codecs a re-encode may target, preferred first: the surround codecs before AAC.
    pub(crate) fn transcode_audio_codecs(&self) -> Vec<&'static str> {
        ["ac3", "eac3", "aac", "dts"].into_iter().filter(|c| self.device.audio_has(c)).collect()
    }

    /// Whether the panel takes Dolby Vision, as the webOS port probed it.
    pub(crate) fn dolby_vision(&self) -> DvCapability {
        self.dolby_vision
    }

    /// HDR10 output, as the panel's configd reports it (`tv.model.supportHDR`).
    pub(crate) fn hdr10(&self) -> Support {
        match self.hdr {
            HdrCapability::Supported => Support::Yes,
            HdrCapability::Unsupported => Support::No,
            HdrCapability::Unknown => Support::Unknown,
        }
    }

    /// HLG output. No platform key reports it, so it follows HDR10 — the official web client's rule
    /// (`supportsHlg ?? supportsHdr10`), and every HDR10 panel LG ships also takes HLG.
    pub(crate) fn hlg(&self) -> Support {
        self.hdr10()
    }

    /// The HEVC `VideoRangeType`s direct play and a video copy may carry, `None` while HDR10 is
    /// not measured (the profile then states no range and the server applies its defaults).
    ///
    /// The Dolby Vision entries mirror the route's own decision (`metadata::Dovi::presentation`):
    /// a panel that takes Dolby Vision is sent Profile 5 and the cross-compatible Profile 8 layers;
    /// one that does not plays a Profile 8 file's base layer, so only the fallbacks it can show are
    /// offered. Profile 7's HDR10 base can be offered on HDR10 panels: the feed converts
    /// eligible dual-layer streams to 8.1. Malformed DV remains excluded, and the item's
    /// precise profile/compatibility fields still gate negotiation before this profile is sent.
    pub(crate) fn hevc_video_ranges(&self) -> Option<Vec<&'static str>> {
        let hdr10 = match self.hdr10() {
            Support::Yes => true,
            Support::No => false,
            Support::Unknown => return None,
        };
        let hlg = self.hlg() == Support::Yes;
        let mut ranges = vec!["SDR"];
        if hdr10 {
            ranges.extend(["HDR10", "HDR10Plus", "DOVIWithEL", "DOVIWithELHDR10Plus"]);
        }
        if hlg {
            ranges.push("HLG");
        }
        if self.dolby_vision == DvCapability::Supported {
            ranges.push("DOVI");
        }
        ranges.push("DOVIWithSDR");
        if hdr10 {
            ranges.extend(["DOVIWithHDR10", "DOVIWithHDR10Plus"]);
        }
        if hlg {
            ranges.push("DOVIWithHLG");
        }
        Some(ranges)
    }

    /// The VP9 `VideoRangeType`s direct play and a video copy may carry, `None` while HDR10 is not
    /// measured. VP9 carries its HDR signalling in the container, not the bitstream, so the panel
    /// learns of it only through `player::vp9_hdr`'s sourceInfo rewrite, which handles PQ and only
    /// on the ACB path (webOS 1-4). HDR10 is therefore offered only where 10-bit VP9 was measured
    /// to reach HDR (`devcaps::Caps::vp9_bit_depth` = 10); HLG never, as nothing signals it.
    pub(crate) fn vp9_video_ranges(&self) -> Option<Vec<&'static str>> {
        let hdr10 = match self.hdr10() {
            Support::Yes => true,
            Support::No => false,
            Support::Unknown => return None,
        };
        let mut ranges = vec!["SDR"];
        if hdr10 && self.device.vp9_bit_depth >= 10 {
            ranges.push("HDR10");
        }
        Some(ranges)
    }

    /// The AV1 `VideoRangeType`s direct play and a video copy may carry, `None` while HDR10 is not
    /// measured. AV1 states its colour in the sequence header the decoder reads, as HEVC does in its
    /// VUI, so the panel's own HDR10 and HLG support decide it. Profile 10 may be declared
    /// on confirmed DV panels; other panels are offered only compatible base layers.
    /// Actual AV1 Dolby Vision output remains pending device acceptance.
    pub(crate) fn av1_video_ranges(&self) -> Option<Vec<&'static str>> {
        let hdr10 = match self.hdr10() {
            Support::Yes => true,
            Support::No => false,
            Support::Unknown => return None,
        };
        let mut ranges = vec!["SDR"];
        if hdr10 {
            ranges.extend(["HDR10", "HDR10Plus"]);
        }
        if self.hlg() == Support::Yes {
            ranges.push("HLG");
        }
        if self.dolby_vision == DvCapability::Supported {
            ranges.push("DOVI");
        }
        ranges.push("DOVIWithSDR");
        if hdr10 {
            ranges.extend(["DOVIWithHDR10", "DOVIWithHDR10Plus"]);
        }
        if self.hlg() == Support::Yes {
            ranges.push("DOVIWithHLG");
        }
        Some(ranges)
    }

    /// Every frame is decoded by the SoC: the feed hands elementary streams to the platform
    /// decoder and this client has no software video decoder.
    pub(crate) fn hardware_video_decoding(&self) -> Support {
        Support::Yes
    }

    /// Bitstream passthrough to a receiver is the television's audio-output setting, which this
    /// app cannot read; the pipeline hands every track to the platform decoder.
    pub(crate) fn audio_passthrough(&self) -> Support {
        Support::Unknown
    }

    /// One line for the event log: what the profile is built from.
    pub(crate) fn summary(&self) -> String {
        let (w, h) = self.max_resolution();
        let channels: Vec<String> = self.audio_channel_limits().into_iter().map(|(c, ch)| format!("{c}:{ch}")).collect();
        format!(
            "video={} audio={} channels={} containers={} max={w}x{h} bitrate={} dv={} hdr10={} hlg={} hw_decode={} passthrough={} table={}",
            self.video_codecs().join(","),
            self.audio_codecs().join(","),
            if channels.is_empty() { "-".to_string() } else { channels.join(",") },
            CONTAINERS.join(","),
            self.max_bitrate_bps().map_or_else(|| "unbounded".to_string(), |b| b.to_string()),
            self.dolby_vision.compact_display(),
            self.hdr10().label(),
            self.hlg().label(),
            self.hardware_video_decoding().label(),
            self.audio_passthrough().label(),
            if devcaps::measured() { "measured" } else { "assumed" },
        )
    }
}

/// Jellyfin's spelling of one of the pipeline's codec ids.
fn jf_codec(c: &str) -> &str {
    if c == "dca" { "dts" } else { c }
}

/// Whether the renderer draws an embedded subtitle of this codec.
pub fn is_rendered_subtitle(codec: &str) -> bool {
    let codec = codec.to_ascii_lowercase();
    RENDERED_SUBTITLES.split(',').any(|c| c == codec)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn soc(hevc: bool, audio: &str) -> Caps {
        Caps { hevc, audio: audio.into(), ..Caps::assumed() }
    }

    #[test]
    fn direct_play_video_is_the_pipeline_set_the_soc_decodes() {
        let with = soc(true, "aac");
        let without = soc(false, "aac");
        assert_eq!(Capabilities::of(&with, DvCapability::Unknown).video_codecs(), ["h264", "hevc"]);
        assert_eq!(Capabilities::of(&without, DvCapability::Unknown).video_codecs(), ["h264"]);
        assert_eq!(Capabilities::of(&without, DvCapability::Unknown).transcode_video_codecs(), ["h264"]);
        assert_eq!(Capabilities::of(&with, DvCapability::Unknown).transcode_video_codecs(), ["h264", "hevc"]);
    }

    /// VP9 and AV1 are offered for direct play, as copy lanes after the encode head, and with their
    /// own row bounds, exactly where the device table lists their decoders, and nowhere else.
    #[test]
    fn vp9_and_av1_follow_the_device_table() {
        let none = soc(true, "aac");
        let caps = Capabilities::of(&none, DvCapability::Unknown);
        assert_eq!(caps.video_codecs(), ["h264", "hevc"]);
        assert_eq!(caps.pipeline_video_codecs(), ["h264", "hevc"], "Strict Original cannot conjure a decoder");
        assert!(caps.codec_row_bounds().is_empty());

        let both = Caps {
            vp9: true,
            vp9_row: (3840, 2160, 60),
            av1: true,
            av1_row: (1920, 1080, 60),
            ..soc(true, "aac")
        };
        let caps = Capabilities::of(&both, DvCapability::Unknown);
        assert_eq!(caps.video_codecs(), ["h264", "hevc", "vp9", "av1"]);
        assert_eq!(caps.pipeline_video_codecs(), ["h264", "hevc", "vp9", "av1"]);
        let targets = caps.transcode_video_codecs();
        assert_eq!(targets.first(), Some(&"h264"), "the encode head stays H.264");
        assert_eq!(&targets[targets.len() - 2..], ["vp9", "av1"], "copy lanes trail the list");
        assert_eq!(
            caps.codec_row_bounds(),
            [("vp9", 3840, 2160, Some(8)), ("av1", 1920, 1080, None)],
            "a table read alone never claims 10-bit VP9",
        );
    }

    /// VP9 reaches HDR only through the ACB sourceInfo rewrite, so HDR10 is offered only with the
    /// measured 10-bit claim; AV1 follows the panel and the Profile 10 declaration capability.
    #[test]
    fn vp9_and_av1_ranges_follow_what_can_reach_the_panel() {
        let eight = Caps { vp9: true, av1: true, ..Caps::assumed() };
        let ten = Caps { vp9_bit_depth: 10, ..eight.clone() };
        let unknown = Capabilities::of(&eight, DvCapability::Supported);
        assert_eq!(unknown.vp9_video_ranges(), None);
        assert_eq!(unknown.av1_video_ranges(), None);
        let hdr8 = unknown.with_hdr(HdrCapability::Supported);
        assert_eq!(hdr8.vp9_video_ranges().unwrap(), ["SDR"], "8-bit VP9 claims no HDR");
        let hdr10 = Capabilities::of(&ten, DvCapability::Supported).with_hdr(HdrCapability::Supported);
        assert_eq!(hdr10.vp9_video_ranges().unwrap(), ["SDR", "HDR10"]);
        let av1 = hdr10.av1_video_ranges().unwrap();
        assert_eq!(av1, ["SDR", "HDR10", "HDR10Plus", "HLG", "DOVI", "DOVIWithSDR", "DOVIWithHDR10", "DOVIWithHDR10Plus", "DOVIWithHLG"]);
        let sdr = Capabilities::of(&ten, DvCapability::Supported).with_hdr(HdrCapability::Unsupported);
        assert_eq!(sdr.vp9_video_ranges().unwrap(), ["SDR"]);
        assert_eq!(sdr.av1_video_ranges().unwrap(), ["SDR", "DOVI", "DOVIWithSDR"]);
        for capability in [DvCapability::Unsupported, DvCapability::Unknown] {
            let caps = Capabilities::of(&ten, capability).with_hdr(HdrCapability::Supported);
            let ranges = caps.av1_video_ranges().unwrap();
            assert!(!ranges.contains(&"DOVI"));
            assert!(ranges.contains(&"DOVIWithHDR10"));
            assert!(!ranges.iter().any(|r| r.contains("EL") || *r == "DOVIInvalid"));
            let sdr = Capabilities::of(&ten, capability).with_hdr(HdrCapability::Unsupported);
            assert_eq!(sdr.av1_video_ranges().unwrap(), ["SDR", "DOVIWithSDR"]);
        }
    }

    #[test]
    fn audio_is_named_in_jellyfins_spelling() {
        let mut c = soc(true, "aac,ac3,dts");
        c.audio_channels.insert("dts".into(), 6);
        c.audio_channels.insert("truehd".into(), 8);
        let caps = Capabilities::of(&c, DvCapability::Unknown);
        assert_eq!(caps.audio_codecs(), ["aac", "ac3", "dts"]);
        assert_eq!(caps.audio_channel_limits(), [("dts", 6)], "a codec the TV cannot decode has no limit to state");
        assert!(caps.pipeline_audio_codecs().contains(&"dts"));
        let no_row = soc(true, "aac,ac3");
        assert!(!Capabilities::of(&no_row, DvCapability::Unknown).pipeline_audio_codecs().contains(&"dts"), "no DTS row, no DTS");
    }

    #[test]
    fn what_is_not_measured_says_so() {
        let c = Caps::assumed();
        let caps = Capabilities::of(&c, DvCapability::Unknown);
        assert_eq!(caps.hdr10(), Support::Unknown);
        assert_eq!(caps.audio_passthrough(), Support::Unknown);
        assert_eq!(caps.max_bitrate_bps(), None);
        let line = caps.summary();
        assert!(line.contains("hdr10=unknown") && line.contains("table=assumed"), "{line}");
    }

    /// The range list is the panel's: an SDR panel is offered SDR and the Dolby Vision file whose
    /// base layer is SDR, an HDR10 panel the HDR10 family and HLG beside it, and Profile 5 only to
    /// a panel that takes Dolby Vision.
    #[test]
    fn video_ranges_follow_the_measured_panel() {
        let c = Caps::assumed();
        let unknown = Capabilities::of(&c, DvCapability::Unknown);
        assert_eq!(unknown.hevc_video_ranges(), None, "an unmeasured panel states no range");
        let sdr = unknown.with_hdr(HdrCapability::Unsupported);
        assert_eq!(sdr.hevc_video_ranges().unwrap(), ["SDR", "DOVIWithSDR"]);
        let hdr = unknown.with_hdr(HdrCapability::Supported);
        assert_eq!(hdr.hlg(), Support::Yes);
        let ranges = hdr.hevc_video_ranges().unwrap();
        for r in ["SDR", "HDR10", "HDR10Plus", "HLG", "DOVIWithHDR10", "DOVIWithHLG"] {
            assert!(ranges.contains(&r), "{r} missing from {ranges:?}");
        }
        assert!(!ranges.contains(&"DOVI"), "Profile 5 has no base layer to fall back to");
        let dv = Capabilities::of(&c, DvCapability::Supported).with_hdr(HdrCapability::Supported);
        assert!(dv.hevc_video_ranges().unwrap().contains(&"DOVI"));
        for caps in [sdr, hdr, dv] {
            let ranges = caps.hevc_video_ranges().unwrap();
            assert!(!ranges.contains(&"DOVIInvalid"), "{ranges:?}");
            assert_eq!(ranges.contains(&"DOVIWithEL"), caps.hdr10() == Support::Yes);
            assert_eq!(ranges.contains(&"DOVIWithELHDR10Plus"), caps.hdr10() == Support::Yes);
        }
    }

    /// The subtitle formats offered to the server as `Embed` must all be drawable once the track
    /// arrives under the app's own codec name — two lists that used to live in two modules.
    #[test]
    fn every_embedded_format_offered_is_one_the_renderer_draws() {
        for f in EMBEDDED_SUBTITLES {
            let app = crate::jf::convert::codec(f);
            assert!(is_rendered_subtitle(&app), "{f} (app spelling {app}) is offered but not rendered");
        }
        for f in SIDECAR_SUBTITLES.iter().chain(BITMAP_SUBTITLES.iter()) {
            assert!(EMBEDDED_SUBTITLES.contains(f), "{f}");
        }
    }
}
