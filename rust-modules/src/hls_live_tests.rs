//! Contract test of the HLS rung against a REAL Jellyfin server — kept beside `hls` (the media
//! layer), which may name the catalog's `jf` sign-in helpers; the catalog may not name `hls`.
//! Ignored by default; run it the way `jf::live_tests` says.
use crate::catalog::{TranscodeOffset, TranscodeSpec, SectionQuery};
use crate::jf::ids;
use crate::jf::live_tests::{first_bytes, first_section, need_live};

/// An HLS rung asked mid-film, walked the way the player's cursor walks it: the master, its one
/// child, and the segment covering the offset — which the server must produce on request.
#[test]
#[ignore]
fn live_hls_rung_from_an_offset() {
    use crate::hls::{parse_master, parse_media, InheritedAuth, Resource};
    let l = need_live!();
    let c = &l.client;
    let Some(movies) = first_section(c, "movie") else { return };
    let page = c.jf().expect("a Jellyfin seat")
        .section_page(&SectionQuery { section_key: movies, sort: "titleSort", filters: &[], start: 0, size: 1, include_meta: false })
        .expect("page");
    let rk = ids::rating_key(&page.items[0].id);
    let rk = rk.as_str();
    let offset_secs = if c.metadata(rk).expect("detail").duration > 120_000 { 60 } else { 0 };
    let spec = TranscodeSpec {
        rating_key: rk, session: "live-test-hls", encoder_session: "live-test-hls", continues: "",
        contract: crate::catalog::EncodeContract {
            delivery: crate::catalog::TranscodeDelivery::FixedHls { seconds_per_segment: 2 },
            ceiling: Some(crate::catalog::Ceiling { max_kbps: 3000, max_w: 1280, max_h: 720 }),
            ..Default::default()
        },
        audio_stream_id: 0, subtitle_stream_id: 0, offset: TranscodeOffset::from_seconds(offset_secs),
    };
    let enc = c.transcode(&spec).playable().expect("hls transcode");
    let start = crate::catalog::StreamUrl::parse(&enc.url);
    let plain = start.path.split('?').next().unwrap_or_default();
    eprintln!("hls: {} video {}->{} audio {}->{} master {}", enc.method.as_str(),
        enc.video.source, enc.video.output, enc.audio.source, enc.audio.output, plain.ends_with("/master.m3u8"));
    assert!(plain.ends_with("/master.m3u8"));
    assert!(!enc.url.contains("StartTimeTicks="), "an HLS master carries no start");

    let get = |r: &Resource, auth: &InheritedAuth| {
        let path = auth.request_path(r).expect("same credential");
        let reply = crate::http::request_bulk(c.origin(), &path, crate::http::Method::Get, &[], None).expect("reply");
        assert_eq!(reply.status, 200, "{}", r.path.split('?').next().unwrap_or_default());
        String::from_utf8(reply.body).expect("text")
    };
    let master_at = Resource::new(c.origin().clone(), &start.path).expect("master resource");
    let auth = InheritedAuth::capture(&master_at).expect("ApiKey");
    let master = parse_master(&master_at, &get(&master_at, &auth)).expect("master parses");
    let media = parse_media(&master.variant.resource, &get(&master.variant.resource, &auth)).expect("media parses");
    let (index, start_ns) = media.start_by_time(offset_secs * 1_000_000).expect("start");
    eprintln!("hls: {} segments, start tag {}, offset {offset_secs}s opens segment {index} at {}ms",
        media.segments.len(), media.start_offset_micros.is_some(), start_ns / 1_000_000);
    assert!(media.end_list && media.start_offset_micros.is_none());
    assert!(start_ns <= offset_secs * 1_000_000_000);
    let segment = &media.segments[index];
    assert!(start_ns + segment.duration.as_nanos() as i64 > offset_secs * 1_000_000_000);
    let first = first_bytes(c.origin(), &auth.request_path(&segment.resource).expect("same credential"));
    eprintln!("hls segment first bytes: {:?}", first.as_ref().map(|(s, n)| (s.as_str(), n)));
    assert!(first.is_some_and(|(s, n)| s.contains(" 200 ") && n > 0));
    assert!(c.transcode_stop("live-test-hls"));
}
