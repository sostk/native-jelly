//! Contract tests against a REAL Jellyfin server, through the same `plex::Client` facade the app
//! reads. Ignored by default; run with
//!
//! ```text
//! JF_URL=http://127.0.0.1:8096 JF_USER=… JF_PASS=… cargo test --lib jf::live_tests -- --ignored --test-threads=1
//! ```
//!
//! Use a dedicated TEST account: the playback test writes progress and watched state. The tests
//! print counts and verdicts only — never titles, paths or tokens (the library is private).
use super::{auth, ids, seat};
use crate::catalog::{
    Client, Origin, SectionQuery, TimelineReport, TimelineState, TranscodeOffset, TranscodeSpec,
};

const CLIENT_ID: &str = "nativejelly-jf-live-test";

struct Live {
    client: Client,
}

fn live() -> Option<Live> {
    let url = std::env::var("JF_URL").ok()?;
    let user = std::env::var("JF_USER").ok()?;
    let pass = std::env::var("JF_PASS").unwrap_or_default();
    let origin = Origin::parse(&url)?;
    let s = auth::sign_in_with_password(&origin, CLIENT_ID, &user, &pass).expect("sign-in");
    seat::register_with(&origin, s.seat());
    Some(Live { client: crate::catalog::unregistered_client(origin, &s.token, CLIENT_ID) })
}

macro_rules! need_live {
    () => {
        match live() {
            Some(l) => l,
            None => {
                eprintln!("JF_URL/JF_USER unset — skipping");
                return;
            }
        }
    };
}

/// `(status line, body bytes after the headers)` from the first 64 KiB of a plaintext GET.
fn first_bytes(origin: &Origin, path: &str) -> Option<(String, usize)> {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect((origin.host(), origin.port() as u16)).ok()?;
    s.set_read_timeout(Some(std::time::Duration::from_secs(60))).ok()?;
    write!(s, "GET {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n", origin.authority()).ok()?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    while buf.len() < 64 * 1024 {
        let n = s.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            if buf.len() > at + 4 + 1024 {
                break;
            }
        }
    }
    let at = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
    let status = String::from_utf8_lossy(&buf[..buf.iter().position(|&b| b == b'\r')?]).into_owned();
    Some((status, buf.len() - at - 4))
}

fn first_section(c: &Client, kind: &str) -> Option<i64> {
    c.sections()?.directory.into_iter().find(|d| d.kind == kind).and_then(|d| d.key.parse().ok())
}

#[test]
#[ignore]
fn live_sections_page_sort_and_filter() {
    let l = need_live!();
    let c = &l.client;
    let secs = c.sections().expect("sections");
    eprintln!("sections: {}", secs.directory.len());
    assert!(!secs.directory.is_empty());
    let Some(movies) = first_section(c, "movie") else { return };
    let page = c
        .section_items_query(&SectionQuery { section_key: movies, sort: "addedAt:desc", filters: &[], start: 0, size: 5, include_meta: true })
        .expect("page");
    eprintln!("movies: total {} page {}", page.total_size, page.metadata.len());
    assert!(page.total_size >= page.metadata.len() as i64);
    assert!(page.meta.as_ref().is_some_and(|m| !m.types[0].sort.is_empty()));
    let m = &page.metadata[0];
    assert!(!m.rating_key.is_empty() && m.rating_key.parse::<i64>().is_ok());
    assert!(m.thumb.starts_with("/library/metadata/"), "artwork is Plex-shaped");
    let img = c.image_transcode_path(&m.thumb, 300, 450, false);
    assert!(img.starts_with("/Items/") && img.contains("/Images/Primary"), "and translates to Jellyfin");
    let bytes = c.fetch_built(&img).expect("poster bytes");
    assert!(bytes.len() > 1000);

    let genres = c.section_directory(movies, "genre", None).expect("genres");
    eprintln!("genres: {}", genres.directory.len());
    if let Some(g) = genres.directory.first() {
        let f = vec![("genre".to_string(), g.key.clone())];
        let p = c.section_items_query(&SectionQuery { section_key: movies, sort: "titleSort", filters: &f, start: 0, size: 50, include_meta: false }).expect("genre page");
        assert!(p.total_size <= page.total_size);
    }
    let letters = c.section_directory(movies, "firstCharacter", None).expect("letters");
    let n: i64 = letters.directory.iter().map(|d| d.size).sum();
    assert_eq!(n, page.total_size, "the letter index covers the whole library");
}

#[test]
#[ignore]
fn live_home_detail_and_search() {
    let l = need_live!();
    let c = &l.client;
    let hubs = c.home_hubs(12).expect("home hubs");
    eprintln!("home hubs: {:?}", hubs.hub.iter().map(|h| (h.hub_identifier.as_str(), h.size)).collect::<Vec<_>>());
    let cw = c.continue_watching(12).expect("continue watching");
    assert_eq!(cw.hub.len(), 1);
    let _ = c.promoted(10);
    if let Some(sec) = first_section(c, "movie") {
        let lh = c.library_hubs(sec, 10).expect("library hubs");
        eprintln!("library hubs: {:?}", lh.hub.iter().map(|h| (h.hub_identifier.as_str(), h.size)).collect::<Vec<_>>());
    }
    let any = hubs.hub.iter().flat_map(|h| h.metadata.iter()).next().map(|m| m.rating_key.clone());
    let Some(rk) = any else { return };
    let d = c.metadata(&rk).expect("detail");
    eprintln!("detail: kind {} media {} markers {} roles {}", d.kind, d.media.len(), d.marker.len(), d.role.len());
    match d.kind.as_str() {
        "show" => {
            let seasons = c.children(&d.rating_key).expect("seasons");
            assert!(!seasons.metadata.is_empty());
            let eps = c.children(&seasons.metadata[0].rating_key).expect("episodes");
            assert!(eps.metadata.iter().all(|e| e.kind == "episode"));
            let all = c.all_leaves(&d.rating_key).expect("all leaves");
            assert!(all.metadata.len() >= eps.metadata.len());
        }
        "movie" => {
            assert!(!d.media.is_empty(), "a movie detail carries its media");
            let p = &d.media[0].part[0];
            assert!(p.key.starts_with("/Videos/") && p.key.contains("static=true"));
        }
        _ => {}
    }
    let _ = c.related(&d.rating_key).expect("related");
    let _ = c.extras(&d.rating_key).expect("extras");
    let q: String = d.title.chars().take(4).collect();
    let s = c.search(&q, 10, 0).expect("search");
    let hits: i64 = s.hub.iter().map(|h| h.size).sum();
    eprintln!("search hubs {} hits {hits}", s.hub.len());
    assert!(hits > 0, "an item's own title prefix finds something");
}

#[test]
#[ignore]
fn live_playback_decisions_reports_and_watched_state() {
    let l = need_live!();
    let c = &l.client;
    let Some(movies) = first_section(c, "movie") else { return };
    let page = c
        .section_items_query(&SectionQuery { section_key: movies, sort: "titleSort", filters: &[], start: 0, size: 1, include_meta: false })
        .expect("page");
    let rk = page.metadata[0].rating_key.clone();
    let d = c.metadata(&rk).expect("detail");
    let part = d.media[0].part[0].key.clone();

    let session = "live-test-session";
    let mde = c.mde_decision(&rk, session, 0, 0).expect("mde");
    let p = mde.metadata[0].first_part().expect("part");
    eprintln!("mde: code {:?} part {} streams {:?}", mde.general_decision_code, p.decision,
        p.stream.iter().map(|s| (s.stream_type, s.codec.as_str(), s.decision.as_str())).collect::<Vec<_>>());
    assert_eq!(mde.general_decision_code, Some(1000));

    let url = c.direct_play_url(&part, session);
    assert!(url.path.contains("PlaySessionId=") && super::url::has_api_key(&url.path));
    // The ranged GET the player makes first.
    let r = crate::http::request_bulk(c.origin(), &format!("{}", url.path), crate::http::Method::Get, &["Range: bytes=0-1023"], None).expect("range");
    assert!(r.status == 206 || r.status == 200, "direct play answered {}", r.status);

    for (t, state) in [(1000, TimelineState::Playing), (6000, TimelineState::Paused), (9000, TimelineState::Stopped)] {
        let ok = c.timeline(&TimelineReport { rating_key: &rk, state, time_ms: t, duration_ms: d.duration, session,
            play_queue_id: "", play_queue_item_id: "", audio_stream_id: 0, subtitle_stream_id: 0 });
        assert!(ok, "{} report", state.as_str());
    }

    let spec = TranscodeSpec {
        rating_key: &rk, session: "live-test-encode", encoder_session: "live-test-encode",
        contract: crate::catalog::EncodeContract { remux: false, no_video_copy: true,
            ceiling: Some(crate::catalog::Ceiling { max_kbps: 3000, max_w: 1280, max_h: 720 }), ..Default::default() },
        audio_stream_id: 0, subtitle_stream_id: 0, offset: TranscodeOffset::from_seconds(30),
    };
    let dec = c.transcode_decision(&spec).expect("transcode decision");
    let dp = dec.metadata[0].first_part().unwrap();
    eprintln!("encode: code {:?} streams {:?}", dec.general_decision_code,
        dp.stream.iter().map(|s| (s.stream_type, s.codec.as_str(), s.decision.as_str())).collect::<Vec<_>>());
    assert_eq!(dp.stream[0].decision, "transcode");
    let start = c.transcode_start_url(&spec);
    assert!(start.path.contains("/stream.mkv") && start.path.contains("StartTimeTicks=300000000"));
    // A live encode never ends, so read the status line and the first bytes off a raw socket.
    let first = first_bytes(c.origin(), &start.path);
    eprintln!("encode first bytes: {:?}", first.as_ref().map(|(s, n)| (s.as_str(), n)));
    assert!(first.is_some_and(|(s, n)| s.contains(" 200 ") && n > 0));
    assert!(c.transcode_stop("live-test-encode"));

    let pq = c.create_play_queue("", &rk, session, true).expect("queue");
    assert_eq!(pq.items.len(), 1, "a movie's queue is itself");

    assert!(c.scrobble(&rk));
    assert!(c.metadata(&rk).unwrap().view_count > 0);
    assert!(c.unscrobble(&rk));
    assert_eq!(c.metadata(&rk).unwrap().view_count, 0);
    assert!(c.remove_from_continue_watching(&rk));
    let _ = ids::guid_of_key(&rk).expect("interned");
}
