//! Jellyfin artwork addressing: which image an item draws, and the request that fetches it.
//!
//! An item's artwork is held as `/library/metadata/{key}/{thumb|art|clearLogo}/{guid}-{tag}`, the
//! path shape the poster store keys its memo, disk cache and cold-open Home cache by; the GUID
//! rides in the tag segment so a path persisted by an earlier run still resolves. [`jf_image_path`]
//! turns one into `/Items/{id}/Images/{Primary|Backdrop|Logo}` at request time.
use super::ids;
use super::models::BaseItemDto;
use crate::catalog::{HexColor, UltraBlurColors};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Which item's Logo image stands for an interned key — an episode or season has none of its own
/// and borrows its series', which `/library/metadata/{rk}/clearLogo` (built by the poster store
/// from a key alone) cannot say.
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

/// Record whose Logo `it`'s key draws: its own, else its parent's or series'.
pub fn note_logo(it: &BaseItemDto) {
    let rk = ids::intern(&it.id);
    if it.image_tags.contains_key("Logo") {
        note_logo_owner(rk, &it.id);
    } else if let Some(owner) = it.parent_logo_item_id.as_deref().or(it.series_id.as_deref()) {
        note_logo_owner(rk, owner);
    }
}

/// `/library/metadata/{rk}/{slot}/{guid}-{tag}` for `guid`'s image, `""` without a tag.
pub fn art_path(guid: &str, slot: &str, tag: Option<&str>) -> String {
    match (ids::intern(guid), tag) {
        (rk, Some(tag)) if rk != 0 && !tag.is_empty() => {
            format!("/library/metadata/{rk}/{slot}/{}-{tag}", ids::normalize(guid))
        }
        _ => String::new(),
    }
}

/// The item's own Primary image, `""` when it has none.
pub fn primary(it: &BaseItemDto) -> String {
    art_path(&it.id, "thumb", it.image_tags.get("Primary").map(String::as_str))
}

/// The series' Primary image an episode or season carries, `""` when there is none.
pub fn series_primary(it: &BaseItemDto) -> String {
    art_path(it.series_id.as_deref().unwrap_or(""), "thumb", it.series_primary_image_tag.as_deref())
}

/// The poster an item wears as itself: its own Primary, or a season without one its series'.
pub fn thumb(it: &BaseItemDto) -> String {
    let own = primary(it);
    if own.is_empty() && it.kind == "Season" { series_primary(it) } else { own }
}

/// The item's backdrop, else the one it inherits from its parent (an episode's series).
pub fn backdrop(it: &BaseItemDto) -> String {
    match it.backdrop_image_tags.first() {
        Some(tag) => art_path(&it.id, "art", Some(tag)),
        None => match (&it.parent_backdrop_item_id, it.parent_backdrop_image_tags.first()) {
            (Some(owner), Some(tag)) => art_path(owner, "art", Some(tag)),
            _ => String::new(),
        },
    }
}

/// A person's portrait as a search hit carries it: keyed by the person's interned id alone.
pub fn person_thumb(p: &BaseItemDto) -> String {
    p.image_tags.get("Primary").map(|t| format!("/library/metadata/{}/thumb/{t}", ids::intern(&p.id)))
        .unwrap_or_default()
}

/// `{guid}-{tag}` → (`guid`, `tag`); a bare tag (an app-built path) → (`None`, `tag`).
pub(crate) fn split_art_tag(seg: &str) -> (Option<&str>, &str) {
    match seg.split_at_checked(32) {
        Some((g, rest)) if rest.starts_with('-') && g.bytes().all(|b| b.is_ascii_hexdigit()) => (Some(g), &rest[1..]),
        _ => (None, seg),
    }
}

/// The Jellyfin image request for an artwork path, or `None` when `src` is not one.
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
        q.push_str(&crate::catalog::urlenc_str(tag));
    }
    if png {
        q.push_str("&format=Png");
    }
    Some(q)
}

/// The four ambient-wash corners from the backdrop's BlurHash, else the poster's.
pub fn ultra_blur(it: &BaseItemDto) -> Option<UltraBlurColors> {
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
