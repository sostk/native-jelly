//! Jellyfin DTOs → the Plex-shaped records every store and screen already reads.
//!
//! **The one translation layer.** Everything above `plex::Client` keeps reading `Metadata`,
//! `Media`, `Stream`, `Hub` and `LibrarySection`; this file is where a `BaseItemDto` becomes one,
//! and the rules it encodes are the port's semantic decisions (docs/jellyfin-port.md):
//!
//! * identities are interned ([`super::ids`]) — `ratingKey`, section keys and person ids stay
//!   decimal integers;
//! * time is ticks on the wire and milliseconds here ([`super::ticks`]);
//! * artwork is emitted as the Plex-SHAPED path `/library/metadata/{rk}/{thumb|art}/{tag}` so the
//!   poster store's keys, memo and caches work unchanged, and `jf_image_path` translates it to
//!   `/Items/{id}/Images/{type}` at request time;
//! * a part key IS the Jellyfin direct-play path, so `Client::direct_play_url` only has to append
//!   the play session and the credential;
//! * track ids are `Index + 1`, so `0` keeps meaning "off".
use super::models::*;
use super::{ids, ticks};
use crate::plex::{
    Chapter, Hub, HexColor, LibrarySection, Marker, Media, MediaContainer, MediaPart, Metadata,
    Rating, Stream, Tag, UltraBlurColors,
};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// `Type` → the Plex `type` the app switches on.
pub fn kind(jf_type: &str) -> &'static str {
    match jf_type {
        "Movie" => "movie",
        "Series" => "show",
        "Season" => "season",
        "Episode" => "episode",
        "BoxSet" => "collection",
        "Person" => "person",
        "Trailer" | "Video" | "MusicVideo" => "clip",
        _ => "",
    }
}

/// The inverse for `IncludeItemTypes`.
pub fn jf_type(plex_kind: &str) -> Option<&'static str> {
    Some(match plex_kind {
        "movie" => "Movie",
        "show" => "Series",
        "season" => "Season",
        "episode" => "Episode",
        "collection" => "BoxSet",
        _ => return None,
    })
}

/// Plex numeric metadata type (`type=1` movie, `2` show, `3` season, `4` episode, `18` collection).
pub fn jf_type_of_number(n: i64) -> Option<&'static str> {
    Some(match n {
        1 => "Movie",
        2 => "Series",
        3 => "Season",
        4 => "Episode",
        18 => "BoxSet",
        _ => return None,
    })
}

fn is_folder_kind(k: &str) -> bool {
    matches!(k, "show" | "season" | "collection")
}

/// Which item's Logo image stands for an interned key — an episode or season has none of its own
/// and borrows its series', which `/library/metadata/{rk}/clearLogo` (built by the poster store
/// from a ratingKey alone) cannot say.
fn logo_owners() -> &'static Mutex<HashMap<i64, String>> {
    static T: OnceLock<Mutex<HashMap<i64, String>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn logo_owner(rk: i64) -> Option<String> {
    logo_owners().lock().ok()?.get(&rk).cloned()
}

fn note_logo_owner(rk: i64, owner: &str) {
    if rk == 0 || owner.is_empty() {
        return;
    }
    if let Ok(mut t) = logo_owners().lock() {
        t.insert(rk, ids::normalize(owner));
    }
}

/// `/library/metadata/{rk}/{slot}/{guid}-{tag}` for `guid`'s image, `""` without a tag. The
/// GUID rides in the tag segment because these paths outlive the process (the cold-open Home
/// cache, the poster store's keys), and `rk` alone resolves only in the process that minted it.
fn art_path(guid: &str, slot: &str, tag: Option<&str>) -> String {
    match (ids::intern(guid), tag) {
        (rk, Some(tag)) if rk != 0 && !tag.is_empty() => {
            format!("/library/metadata/{rk}/{slot}/{}-{tag}", ids::normalize(guid))
        }
        _ => String::new(),
    }
}

/// `{guid}-{tag}` → (`guid`, `tag`); a bare tag (an app-built path) → (`None`, `tag`).
fn split_art_tag(seg: &str) -> (Option<&str>, &str) {
    match seg.split_at_checked(32) {
        Some((g, rest)) if rest.starts_with('-') && g.bytes().all(|b| b.is_ascii_hexdigit()) => (Some(g), &rest[1..]),
        _ => (None, seg),
    }
}

/// The Jellyfin image request for a Plex-shaped artwork path, or `None` when `src` is not one.
/// Anonymous on every supported server (measured: 200 with no credential on 12.0), so no token.
pub fn jf_image_path(src: &str, w: i64, h: i64, png: bool) -> Option<String> {
    let rest = src.strip_prefix("/library/metadata/")?;
    let mut parts = rest.splitn(3, '/');
    let rk: i64 = parts.next()?.parse().ok()?;
    let slot = parts.next()?;
    let (carried, tag) = match parts.next().filter(|t| !t.is_empty()).map(split_art_tag) {
        Some((g, t)) => (g.map(str::to_string), Some(t).filter(|t| !t.is_empty())),
        None => (None, None),
    };
    let image_type = match slot {
        "thumb" => "Primary",
        "art" => "Backdrop",
        "clearLogo" => "Logo",
        "thumbLand" => "Thumb",
        "banner" => "Banner",
        _ => return None,
    };
    let owner = if slot == "clearLogo" { logo_owner(rk) } else { None }
        .or(carried)
        .or_else(|| ids::guid_of(rk))?;
    let mut q = format!("/Items/{owner}/Images/{image_type}?maxWidth={w}&maxHeight={h}&quality=90");
    if let Some(tag) = tag {
        q.push_str("&tag=");
        q.push_str(&crate::plex::urlenc_str(tag));
    }
    if png {
        q.push_str("&format=Png");
    }
    Some(q)
}

/// `2024-05-01T12:34:56.1234567Z` → unix seconds; 0 for anything unparseable (and for the
/// `0001-01-01` sentinel Jellyfin sends for "never").
pub fn iso_to_unix(s: &str) -> i64 {
    let b = s.as_bytes();
    if b.len() < 19 {
        return 0;
    }
    let num = |r: std::ops::Range<usize>| s.get(r).and_then(|x| x.parse::<i64>().ok());
    let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(se)) =
        (num(0..4), num(5..7), num(8..10), num(11..13), num(14..16), num(17..19))
    else {
        return 0;
    };
    if y < 1971 {
        return 0;
    }
    // days from civil (Howard Hinnant)
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    days * 86_400 + h * 3600 + mi * 60 + se
}

/// ISO 639-2 (bibliographic or terminology) → (ISO 639-1, English name).
fn language(code: &str) -> Option<(&'static str, &'static str)> {
    const T: &[(&str, &str, &str, &str)] = &[
        ("eng", "eng", "en", "English"), ("fre", "fra", "fr", "French"), ("ger", "deu", "de", "German"),
        ("spa", "spa", "es", "Spanish"), ("ita", "ita", "it", "Italian"), ("por", "por", "pt", "Portuguese"),
        ("dut", "nld", "nl", "Dutch"), ("swe", "swe", "sv", "Swedish"), ("nor", "nor", "no", "Norwegian"),
        ("nob", "nob", "nb", "Norwegian Bokmål"), ("dan", "dan", "da", "Danish"), ("fin", "fin", "fi", "Finnish"),
        ("ice", "isl", "is", "Icelandic"), ("pol", "pol", "pl", "Polish"), ("cze", "ces", "cs", "Czech"),
        ("slo", "slk", "sk", "Slovak"), ("slv", "slv", "sl", "Slovenian"), ("hun", "hun", "hu", "Hungarian"),
        ("rum", "ron", "ro", "Romanian"), ("bul", "bul", "bg", "Bulgarian"), ("hrv", "hrv", "hr", "Croatian"),
        ("srp", "srp", "sr", "Serbian"), ("gre", "ell", "el", "Greek"), ("tur", "tur", "tr", "Turkish"),
        ("rus", "rus", "ru", "Russian"), ("ukr", "ukr", "uk", "Ukrainian"), ("est", "est", "et", "Estonian"),
        ("lav", "lav", "lv", "Latvian"), ("lit", "lit", "lt", "Lithuanian"), ("heb", "heb", "he", "Hebrew"),
        ("ara", "ara", "ar", "Arabic"), ("per", "fas", "fa", "Persian"), ("hin", "hin", "hi", "Hindi"),
        ("tam", "tam", "ta", "Tamil"), ("tel", "tel", "te", "Telugu"), ("kan", "kan", "kn", "Kannada"),
        ("mal", "mal", "ml", "Malayalam"), ("ben", "ben", "bn", "Bengali"), ("tha", "tha", "th", "Thai"),
        ("vie", "vie", "vi", "Vietnamese"), ("ind", "ind", "id", "Indonesian"), ("may", "msa", "ms", "Malay"),
        ("fil", "fil", "tl", "Filipino"), ("chi", "zho", "zh", "Chinese"), ("jpn", "jpn", "ja", "Japanese"),
        ("kor", "kor", "ko", "Korean"), ("cat", "cat", "ca", "Catalan"), ("baq", "eus", "eu", "Basque"),
        ("glg", "glg", "gl", "Galician"), ("aze", "aze", "az", "Azerbaijani"), ("kaz", "kaz", "kk", "Kazakh"),
        ("kir", "kir", "ky", "Kyrgyz"), ("sin", "sin", "si", "Sinhala"), ("urd", "urd", "ur", "Urdu"),
        ("afr", "afr", "af", "Afrikaans"), ("alb", "sqi", "sq", "Albanian"), ("arm", "hye", "hy", "Armenian"),
        ("geo", "kat", "ka", "Georgian"), ("mac", "mkd", "mk", "Macedonian"), ("bos", "bos", "bs", "Bosnian"),
        ("wel", "cym", "cy", "Welsh"), ("gle", "gle", "ga", "Irish"), ("lat", "lat", "la", "Latin"),
    ];
    let c = code.trim().to_ascii_lowercase();
    T.iter().find(|(b, t, one, _)| *b == c || *t == c || *one == c).map(|(_, _, one, name)| (*one, *name))
}

/// Jellyfin codec spellings → the PMS ones the routing and track UI were written against.
pub fn codec(jf: &str) -> String {
    let c = jf.to_ascii_lowercase();
    match c.as_str() {
        "dts" => "dca".into(),
        "subrip" => "srt".into(),
        "pgssub" | "hdmv_pgs_subtitle" => "pgs".into(),
        "dvdsub" | "dvd_subtitle" => "vobsub".into(),
        "webvtt" => "vtt".into(),
        "ssa" => "ssa".into(),
        "mpeg2video" => "mpeg2video".into(),
        _ => c,
    }
}

fn stream_type(kind: &str) -> i64 {
    match kind {
        "Video" => 1,
        "Audio" => 2,
        "Subtitle" => 3,
        _ => 0,
    }
}

/// One `MediaStream` → `Stream`. `item_guid`/`source_id` build an external subtitle's key.
pub fn stream(s: &MediaStream, item_guid: &str, source_id: &str) -> Stream {
    let st = stream_type(&s.kind);
    let lang = s.language.as_deref().unwrap_or("");
    let (one, name) = language(lang).unwrap_or(("", ""));
    let range = s.video_range_type.as_deref().unwrap_or("");
    let dovi = range.starts_with("DOVI");
    let key = if st == 3 && s.is_external {
        let fmt = codec(s.codec.as_deref().unwrap_or("srt"));
        format!("/Videos/{}/{}/Subtitles/{}/0/Stream.{fmt}", ids::normalize(item_guid), ids::normalize(source_id), s.index)
    } else {
        String::new()
    };
    Stream {
        id: ids::track_id(s.index),
        stream_type: st,
        index: s.index,
        key,
        codec: codec(s.codec.as_deref().unwrap_or("")),
        language: if name.is_empty() { lang.to_string() } else { name.to_string() },
        language_code: if lang.is_empty() { String::new() } else { lang.to_ascii_lowercase() },
        language_tag: one.to_string(),
        frame_rate: s.real_frame_rate.or(s.average_frame_rate).unwrap_or(0.0),
        color_trc: s.color_transfer.clone().unwrap_or_default(),
        dovi_present: dovi as i64,
        // Jellyfin reports the range type, not the RPU profile fields; a `DOVIWith*` source has a
        // self-displayable base layer (compat id 1 = HDR10, 2 = SDR, 4 = HLG), plain `DOVI` is P5.
        dovi_profile: match range { "DOVI" => 5, "DOVIWithHDR10" | "DOVIWithSDR" | "DOVIWithHLG" => 8, _ => 0 },
        dovi_bl_compat_id: match range { "DOVIWithHDR10" => 1, "DOVIWithSDR" => 2, "DOVIWithHLG" => 4, _ => 0 },
        dovi_el_present: (range == "DOVIWithEL") as i64,
        bitrate: s.bit_rate.unwrap_or(0) / 1000,
        profile: s.profile.clone().unwrap_or_default().to_ascii_lowercase(),
        bit_depth: s.bit_depth.unwrap_or(0),
        channels: s.channels.unwrap_or(0),
        audio_channel_layout: s.channel_layout.clone().unwrap_or_default(),
        display_title: s.display_title.clone().unwrap_or_default(),
        title: s.title.clone().unwrap_or_default(),
        hearing_impaired: s.is_hearing_impaired as i64,
        forced: s.is_forced as i64,
        is_default: s.is_default as i64,
        ..Default::default()
    }
}

fn resolution_class(w: i64, h: i64) -> String {
    match (w, h) {
        (w, h) if w >= 3200 || h >= 1800 => "4k".into(),
        (w, h) if w >= 1800 || h >= 1000 => "1080".into(),
        (w, h) if w >= 1200 || h >= 700 => "720".into(),
        (_, h) if h >= 560 => "576".into(),
        (_, h) if h > 0 => "sd".into(),
        _ => String::new(),
    }
}

/// The direct-play path for one media source — `Part.key`. `static=true` asks for the file's
/// own bytes with HTTP Range (measured: 206 on 12.0); the play session and `ApiKey` are added by
/// `Client::direct_play_url`.
pub fn part_key(item_guid: &str, source: &MediaSourceInfo) -> String {
    let ext = source.container.as_deref().unwrap_or("mkv").split(',').next().unwrap_or("mkv");
    format!(
        "/Videos/{}/stream.{ext}?static=true&MediaSourceId={}",
        ids::normalize(item_guid),
        ids::normalize(&source.id)
    )
}

/// One `MediaSourceInfo` → one `Media` version with its one `Part`.
pub fn media(item_guid: &str, source: &MediaSourceInfo) -> Media {
    let video = source.media_streams.iter().find(|s| s.kind == "Video");
    let audio_idx = source.default_audio_stream_index;
    let audio = source
        .media_streams
        .iter()
        .filter(|s| s.kind == "Audio")
        .find(|s| Some(s.index) == audio_idx)
        .or_else(|| source.media_streams.iter().find(|s| s.kind == "Audio"));
    let (w, h) = video.map(|v| (v.width.unwrap_or(0), v.height.unwrap_or(0))).unwrap_or((0, 0));
    let container = source.container.clone().unwrap_or_default();
    Media {
        video_codec: video.and_then(|v| v.codec.as_deref()).map(codec).unwrap_or_default(),
        audio_codec: audio.and_then(|a| a.codec.as_deref()).map(codec).unwrap_or_default(),
        bitrate: source.bitrate.unwrap_or(0) / 1000,
        width: w,
        height: h,
        video_resolution: resolution_class(w, h),
        container: container.clone(),
        aspect_ratio: if h > 0 { (w as f64 / h as f64 * 100.0).round() / 100.0 } else { 0.0 },
        video_profile: video.and_then(|v| v.profile.clone()).unwrap_or_default().to_ascii_lowercase(),
        part: vec![MediaPart {
            id: ids::intern(&source.id),
            key: part_key(item_guid, source),
            file: source.path.clone().unwrap_or_default(),
            size: source.size.unwrap_or(0),
            container,
            stream: source.media_streams.iter().filter(|s| stream_type(&s.kind) != 0)
                .map(|s| stream(s, item_guid, &source.id)).collect(),
            ..Default::default()
        }],
    }
}

fn person_tag(p: &BaseItemPerson) -> Tag {
    Tag {
        tag: p.name.clone(),
        role: p.role.clone().unwrap_or_default(),
        thumb: art_path(&p.id, "thumb", p.primary_image_tag.as_deref()),
        id: ids::intern(&p.id),
        tag_key: ids::normalize(&p.id),
        ..Default::default()
    }
}

fn ultra_blur(it: &BaseItemDto) -> Option<UltraBlurColors> {
    let pick = |slot: &str, tag: Option<&str>| {
        let m = it.image_blur_hashes.get(slot)?;
        tag.and_then(|t| m.get(t)).or_else(|| m.values().next()).cloned()
    };
    let hash = pick("Backdrop", it.backdrop_image_tags.first().map(String::as_str))
        .or_else(|| pick("Primary", it.image_tags.get("Primary").map(String::as_str)))?;
    let c = super::blurhash::corners(&hash)?;
    Some(UltraBlurColors {
        top_left: HexColor(c[0]),
        top_right: HexColor(c[1]),
        bottom_right: HexColor(c[2]),
        bottom_left: HexColor(c[3]),
    })
}

fn ratings(it: &BaseItemDto) -> Vec<Rating> {
    let mut out = Vec::new();
    if let Some(c) = it.critic_rating.filter(|c| *c > 0.0) {
        let state = if c >= 60.0 { "ripe" } else { "rotten" };
        out.push(Rating { image: format!("rottentomatoes://image.rating.{state}"), value: c / 10.0, kind: "critic".into() });
    }
    if let Some(c) = it.community_rating.filter(|c| *c > 0.0) {
        // Jellyfin's community rating comes from the TMDb/IMDb provider the library uses; TMDb is
        // the default metadata provider, so that is the attribution.
        out.push(Rating { image: "themoviedb://image.rating".into(), value: c, kind: "audience".into() });
    }
    out
}

fn extra_type(t: &str) -> (i64, &'static str) {
    match t {
        "Trailer" => (1, "trailer"),
        "DeletedScene" => (2, "deletedScene"),
        "Interview" => (3, "interview"),
        "BehindTheScenes" => (5, "behindTheScenes"),
        "Scene" | "Sample" | "Clip" => (6, "sceneOrSample"),
        "Short" => (11, "short"),
        _ => (10, "featurette"),
    }
}

/// The portable identity "Also available" matches copies by: a provider id when the item has
/// one, the server-local GUID otherwise.
fn portable_guid(it: &BaseItemDto) -> String {
    for (key, scheme) in [("Imdb", "imdb"), ("Tmdb", "tmdb"), ("Tvdb", "tvdb")] {
        if let Some(v) = it.provider_ids.get(key).filter(|v| !v.is_empty()) {
            return format!("{scheme}://{v}");
        }
    }
    format!("jellyfin://{}", ids::normalize(&it.id))
}

/// One item → `Metadata`. `section_id` stamps `librarySectionID` when the caller knows which
/// library the row came from (0 otherwise — an un-gateable row, see `pms::feeds_home_item`).
pub fn item(it: &BaseItemDto, section_id: i64) -> Metadata {
    let k = kind(&it.kind);
    let rk = ids::rating_key(&it.id);
    let rk_num = ids::intern(&it.id);
    let ud = it.user_data.clone().unwrap_or_default();
    let duration = ticks::to_ms(it.run_time_ticks.unwrap_or(0));
    let primary = it.image_tags.get("Primary").map(String::as_str);
    let leaf_count = match k {
        "show" | "season" => it.recursive_item_count.or(it.child_count).unwrap_or(0),
        _ => 0,
    };
    let viewed_leaf_count = match (k, ud.unplayed_item_count) {
        ("show" | "season", Some(unplayed)) => (leaf_count - unplayed).max(0),
        ("show" | "season", None) if ud.played => leaf_count,
        _ => 0,
    };

    let series = it.series_id.as_deref().unwrap_or("");
    let series_thumb = art_path(series, "thumb", it.series_primary_image_tag.as_deref());
    let thumb = match (k, primary) {
        (_, Some(tag)) => art_path(&it.id, "thumb", Some(tag)),
        ("season", None) => series_thumb.clone(),
        _ => String::new(),
    };
    let art = match it.backdrop_image_tags.first() {
        Some(tag) => art_path(&it.id, "art", Some(tag)),
        None => match (&it.parent_backdrop_item_id, it.parent_backdrop_image_tags.first()) {
            (Some(owner), Some(tag)) => art_path(owner, "art", Some(tag)),
            _ => String::new(),
        },
    };
    if it.image_tags.contains_key("Logo") {
        note_logo_owner(rk_num, &it.id);
    } else if let Some(owner) = it.parent_logo_item_id.as_deref().or(it.series_id.as_deref()) {
        note_logo_owner(rk_num, owner);
    }

    let (parent_rk, parent_title, grand_rk, grand_title, parent_thumb, grand_thumb) = match k {
        "season" => (ids::rating_key(series), it.series_name.clone().unwrap_or_default(),
            String::new(), String::new(), series_thumb.clone(), String::new()),
        "episode" => (
            ids::rating_key(it.season_id.as_deref().unwrap_or("")),
            it.season_name.clone().unwrap_or_default(),
            ids::rating_key(series),
            it.series_name.clone().unwrap_or_default(),
            series_thumb.clone(),
            series_thumb.clone(),
        ),
        _ => Default::default(),
    };

    let people = |t: &[&str]| -> Vec<Tag> {
        it.people.iter().filter(|p| t.contains(&p.kind.as_str())).map(person_tag).collect()
    };
    let chapters = {
        let n = it.chapters.len();
        it.chapters.iter().enumerate().map(|(i, c)| Chapter {
            index: i as i64 + 1,
            start_time_offset: ticks::to_ms(c.start_position_ticks),
            end_time_offset: it.chapters.get(i + 1).map(|n| ticks::to_ms(n.start_position_ticks))
                .unwrap_or(duration).max(ticks::to_ms(c.start_position_ticks)),
            tag: c.name.clone().filter(|_| n > 0).unwrap_or_default(),
            thumb: String::new(),
        }).collect()
    };
    let (extra_type_num, subtype) = match it.extra_type.as_deref() {
        Some(t) => { let (n, s) = extra_type(t); (n, s.to_string()) }
        None if k == "clip" => (1, "trailer".to_string()),
        None => (0, String::new()),
    };

    Metadata {
        kind: k.to_string(),
        key: if is_folder_kind(k) { format!("/library/metadata/{rk}/children") } else { format!("/library/metadata/{rk}") },
        rating_key: rk,
        subtype,
        extra_type: extra_type_num,
        guid: portable_guid(it),
        title: it.name.clone(),
        library_section_id: section_id,
        year: it.production_year.unwrap_or(0),
        content_rating: it.official_rating.clone().unwrap_or_default(),
        summary: it.overview.clone().unwrap_or_default(),
        tagline: it.taglines.first().cloned().unwrap_or_default(),
        studio: it.studios.first().map(|s| s.name.clone()).unwrap_or_default(),
        originally_available_at: it.premiere_date.as_deref().and_then(|d| d.get(..10)).unwrap_or("").to_string(),
        duration,
        view_offset: ticks::to_ms(ud.playback_position_ticks),
        last_viewed_at: ud.last_played_date.as_deref().map(iso_to_unix).unwrap_or(0),
        index: it.index_number.unwrap_or(0),
        child_count: it.child_count.unwrap_or(0),
        updated_at: it.date_created.as_deref().map(iso_to_unix).unwrap_or(0),
        parent_index: it.parent_index_number.unwrap_or(0),
        parent_rating_key: parent_rk,
        parent_title,
        grandparent_rating_key: grand_rk,
        grandparent_title: grand_title,
        leaf_count,
        viewed_leaf_count,
        view_count: if ud.played { ud.play_count.max(1) } else { 0 },
        thumb,
        parent_thumb,
        art,
        grandparent_thumb: grand_thumb,
        media: it.media_sources.iter().map(|s| media(&it.id, s)).collect(),
        genre: it.genres.iter().map(|g| Tag { tag: g.clone(), ..Default::default() }).collect(),
        director: people(&["Director"]),
        writer: people(&["Writer"]),
        role: people(&["Actor", "GuestStar"]),
        chapter: chapters,
        ultra_blur_colors: ultra_blur(it),
        ratings: ratings(it),
        rating: it.critic_rating.map(|c| c / 10.0).unwrap_or(0.0),
        audience_rating: it.community_rating.unwrap_or(0.0),
        ..Default::default()
    }
}

/// `MediaSegmentDto[]` → `Marker[]`. Intro → `intro`, Outro → `credits` (final when it reaches
/// the last 2 % of the item); Recap/Preview/Commercial/Unknown have no Plex marker and are dropped.
pub fn markers(segs: &[MediaSegmentDto], duration_ms: i64) -> Vec<Marker> {
    segs.iter()
        .filter_map(|s| {
            let kind = match s.kind.as_str() {
                "Intro" => "intro",
                "Outro" => "credits",
                _ => return None,
            };
            let (start, end) = (ticks::to_ms(s.start_ticks), ticks::to_ms(s.end_ticks));
            (end > start).then(|| Marker {
                kind: kind.into(),
                start_time_offset: start,
                end_time_offset: end,
                is_final: (kind == "credits" && duration_ms > 0 && end >= duration_ms * 98 / 100) as i64,
            })
        })
        .collect()
}

/// A user view → `LibrarySection`, or `None` for a view this client does not browse.
pub fn section(view: &BaseItemDto) -> Option<LibrarySection> {
    let kind = match view.collection_type.as_deref() {
        Some("movies") | Some("homevideos") | Some("musicvideos") => "movie",
        Some("tvshows") => "show",
        _ => return None,
    };
    Some(LibrarySection {
        key: ids::intern(&view.id).to_string(),
        kind: kind.into(),
        title: view.name.clone(),
        size: view.child_count.or(view.recursive_item_count).unwrap_or(0),
    })
}

pub fn container(items: Vec<Metadata>, total: i64, offset: i64) -> MediaContainer {
    MediaContainer {
        size: items.len() as i64,
        total_size: total.max(items.len() as i64),
        offset,
        metadata: items,
        ..Default::default()
    }
}

pub fn hub(identifier: &str, title: &str, kind: &str, key: &str, items: Vec<Metadata>) -> Hub {
    Hub {
        kind: kind.into(),
        hub_identifier: identifier.into(),
        key: key.into(),
        title: title.into(),
        size: items.len() as i64,
        total_size: items.len() as i64,
        metadata: items,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn movie() -> BaseItemDto {
        serde_json::from_str(r#"{
            "Id":"36edf81507b5c8eda566778b2a316c29","Name":"Synthetic Film","Type":"Movie",
            "ProductionYear":2014,"OfficialRating":"PG-13","Overview":"o","Taglines":["t"],
            "RunTimeTicks":81772160000,"CommunityRating":7.1,"CriticRating":73,
            "PremiereDate":"2014-04-10T00:00:00.0000000Z",
            "ProviderIds":{"Imdb":"tt0000001","Tmdb":"1"},
            "ImageTags":{"Primary":"aaaa","Logo":"llll"},"BackdropImageTags":["bbbb"],
            "ImageBlurHashes":{"Backdrop":{"bbbb":"LEHV6nWB2yk8pyo0adR*.7kCMdnj"}},
            "UserData":{"PlaybackPositionTicks":435781990,"PlayCount":0,"Played":false,
                        "LastPlayedDate":"2026-10-01T10:00:00.000Z"},
            "People":[{"Name":"A Person","Id":"9be3b02066ce8ad30a366e955449ec10","Role":"Hero","Type":"Actor","PrimaryImageTag":"pppp"},
                      {"Name":"D Person","Id":"792fd63ffc2432a5fa6a29bfac8b79db","Type":"Director"}],
            "Chapters":[{"StartPositionTicks":0,"Name":"Chapter 01"},{"StartPositionTicks":3000000000,"Name":"Chapter 02"}],
            "MediaSources":[{"Id":"36edf81507b5c8eda566778b2a316c29","Container":"mkv","Size":10,"Bitrate":59256274,
              "DefaultAudioStreamIndex":1,
              "MediaStreams":[
                {"Type":"Video","Index":0,"Codec":"hevc","Width":3840,"Height":1608,"VideoRangeType":"DOVIWithHDR10",
                 "ColorTransfer":"smpte2084","Profile":"Main 10","BitDepth":10,"RealFrameRate":23.976},
                {"Type":"Audio","Index":1,"Codec":"truehd","Language":"eng","Channels":8,"IsDefault":true},
                {"Type":"Audio","Index":2,"Codec":"dts","Language":"ger","Channels":6},
                {"Type":"Subtitle","Index":3,"Codec":"PGSSUB","Language":"fre","IsForced":true},
                {"Type":"Subtitle","Index":4,"Codec":"subrip","Language":"eng","IsExternal":true}
              ]}]
        }"#).unwrap()
    }

    #[test]
    fn a_movie_becomes_the_plex_record_every_screen_reads() {
        let m = item(&movie(), 7);
        assert_eq!(m.kind, "movie");
        assert_eq!(m.rating_key, ids::intern("36edf81507b5c8eda566778b2a316c29").to_string());
        assert_eq!(m.key, format!("/library/metadata/{}", m.rating_key));
        assert_eq!(m.guid, "imdb://tt0000001");
        assert_eq!((m.year, m.duration, m.view_offset), (2014, 8_177_216, 43_578));
        assert_eq!(m.library_section_id, 7);
        assert_eq!(m.originally_available_at, "2014-04-10");
        assert_eq!(m.thumb, format!("/library/metadata/{}/thumb/36edf81507b5c8eda566778b2a316c29-aaaa", m.rating_key));
        assert_eq!(m.art, format!("/library/metadata/{}/art/36edf81507b5c8eda566778b2a316c29-bbbb", m.rating_key));
        assert!(m.ultra_blur_colors.and_then(|u| u.corners()).is_some());
        assert_eq!(m.role.len(), 1);
        assert_eq!(m.role[0].role, "Hero");
        assert_eq!(m.director[0].tag, "D Person");
        assert_eq!(m.chapter[1].start_time_offset, 300_000);
        assert_eq!(m.chapter[0].end_time_offset, 300_000);
        assert_eq!(m.chapter[1].end_time_offset, 8_177_216);
        assert_eq!(m.last_viewed_at, 1_790_848_800);
        assert_eq!(m.ratings.len(), 2);
    }

    #[test]
    fn media_streams_keep_index_order_and_shift_ids_off_zero() {
        let m = item(&movie(), 0);
        let media = &m.media[0];
        assert_eq!((media.video_codec.as_str(), media.audio_codec.as_str()), ("hevc", "truehd"));
        assert_eq!(media.video_resolution, "4k");
        assert_eq!(media.bitrate, 59_256);
        let part = &media.part[0];
        assert!(part.key.starts_with("/Videos/36edf81507b5c8eda566778b2a316c29/stream.mkv?static=true&MediaSourceId="));
        let s = &part.stream;
        assert_eq!(s.iter().map(|x| x.id).collect::<Vec<_>>(), vec![1, 2, 3, 4, 5]);
        assert_eq!(s[0].dovi_present, 1);
        assert_eq!(s[0].dovi_bl_compat_id, 1);
        assert_eq!(s[2].codec, "dca");
        assert_eq!((s[1].language.as_str(), s[1].language_code.as_str(), s[1].language_tag.as_str()), ("English", "eng", "en"));
        assert_eq!(s[3].codec, "pgs");
        assert_eq!(s[3].forced, 1);
        assert!(s[3].key.is_empty(), "embedded subtitles carry no key");
        assert!(s[4].key.ends_with("/Subtitles/4/0/Stream.srt"), "{}", s[4].key);
    }

    #[test]
    fn artwork_paths_translate_to_anonymous_item_images() {
        let m = item(&movie(), 0);
        let p = jf_image_path(&m.thumb, 300, 450, false).unwrap();
        assert_eq!(p, "/Items/36edf81507b5c8eda566778b2a316c29/Images/Primary?maxWidth=300&maxHeight=450&quality=90&tag=aaaa");
        let logo = jf_image_path(&format!("/library/metadata/{}/clearLogo", m.rating_key), 600, 240, true).unwrap();
        assert!(logo.contains("/Images/Logo?") && logo.ends_with("&format=Png"), "{logo}");
        assert!(!p.contains("ApiKey"));
        assert!(jf_image_path("https://metadata-static.plex.tv/x.jpg", 1, 1, false).is_none());
    }

    #[test]
    fn a_persisted_artwork_path_resolves_without_this_process_having_minted_it() {
        let never_minted = "/library/metadata/1/art/eeeeeeeeeeeee00000000000000000ff-bt";
        let p = jf_image_path(never_minted, 10, 10, false).unwrap();
        assert_eq!(p, "/Items/eeeeeeeeeeeee00000000000000000ff/Images/Backdrop?maxWidth=10&maxHeight=10&quality=90&tag=bt");
        assert_eq!(split_art_tag("plain"), (None, "plain"));
    }

    #[test]
    fn an_episode_borrows_its_series_poster_and_logo() {
        let ep: BaseItemDto = serde_json::from_str(r#"{"Id":"aaaaaaaaaaaaa000000000000000000b","Type":"Episode","Name":"E",
            "SeriesId":"bbbbbbbbbbbbb000000000000000000c","SeriesName":"S","SeasonId":"ccccccccccccc000000000000000000d",
            "SeasonName":"Season 1","IndexNumber":3,"ParentIndexNumber":1,"SeriesPrimaryImageTag":"sp",
            "ParentLogoItemId":"bbbbbbbbbbbbb000000000000000000c","ImageTags":{"Primary":"still"}}"#).unwrap();
        let m = item(&ep, 0);
        assert_eq!((m.index, m.parent_index), (3, 1));
        assert_eq!(m.grandparent_title, "S");
        assert_eq!(m.parent_title, "Season 1");
        assert_eq!(m.grandparent_rating_key, ids::intern("bbbbbbbbbbbbb000000000000000000c").to_string());
        assert!(m.grandparent_thumb.ends_with("/thumb/bbbbbbbbbbbbb000000000000000000c-sp"), "{}", m.grandparent_thumb);
        let logo = jf_image_path(&format!("/library/metadata/{}/clearLogo", m.rating_key), 1, 1, true).unwrap();
        assert!(logo.starts_with("/Items/bbbbbbbbbbbbb000000000000000000c/Images/Logo"), "{logo}");
    }

    #[test]
    fn segments_map_to_markers_and_unknown_kinds_are_dropped() {
        let segs: Vec<MediaSegmentDto> = serde_json::from_str(r#"[
            {"Type":"Intro","StartTicks":10000000,"EndTicks":900000000},
            {"Type":"Recap","StartTicks":0,"EndTicks":10000000},
            {"Type":"Outro","StartTicks":30000000000,"EndTicks":31000000000}]"#).unwrap();
        let m = markers(&segs, 3_100_000);
        assert_eq!(m.len(), 2);
        assert_eq!((m[0].kind.as_str(), m[0].start_time_offset, m[0].end_time_offset), ("intro", 1000, 90_000));
        assert_eq!((m[1].kind.as_str(), m[1].is_final), ("credits", 1));
    }

    #[test]
    fn a_series_counts_episodes_for_the_watched_badge() {
        let s: BaseItemDto = serde_json::from_str(r#"{"Id":"ddddddddddddd000000000000000000e","Type":"Series","Name":"S",
            "ChildCount":2,"RecursiveItemCount":20,"UserData":{"UnplayedItemCount":5,"Played":false}}"#).unwrap();
        let m = item(&s, 0);
        assert_eq!(m.kind, "show");
        assert!(m.key.ends_with("/children"));
        assert_eq!((m.child_count, m.leaf_count, m.viewed_leaf_count), (2, 20, 15));
    }

    #[test]
    fn only_browsable_views_become_sections() {
        let v = |t: &str| BaseItemDto { id: "eeeeeeeeeeeee000000000000000000f".into(), name: "L".into(),
            collection_type: Some(t.into()), ..Default::default() };
        assert_eq!(section(&v("movies")).unwrap().kind, "movie");
        assert_eq!(section(&v("tvshows")).unwrap().kind, "show");
        assert!(section(&v("music")).is_none());
        assert!(section(&v("boxsets")).is_none());
    }

    #[test]
    fn iso_dates_and_the_never_sentinel() {
        assert_eq!(iso_to_unix("1970-01-02T00:00:00Z"), 0, "pre-1971 is the sentinel range");
        assert_eq!(iso_to_unix("0001-01-01T00:00:00.0000000Z"), 0);
        assert_eq!(iso_to_unix("2000-03-01T00:00:00Z"), 951_868_800);
        assert_eq!(iso_to_unix("garbage"), 0);
    }
}
