//! Jellyfin identities → the integer identities the app was built around.
//!
//! Every Jellyfin item and library is a 128-bit GUID (`36edf81507b5c8eda566778b2a316c29`, and the
//! dashed spelling of the same value inside a `TranscodingUrl`). The app keys on PMS-shaped ids:
//! `ratingKey` is a decimal string that `plex::Client::show_language_prefs` and the replay tapes
//! expect to be digits, and a library is an `i64` section key that `HomePins` PERSISTS. So the two
//! spaces are bridged here, and nowhere else.
//!
//! **The mapping is a pure function of the GUID** — the first 13 hex digits (52 bits) read as an
//! integer. Pure, so a pin written on Monday names the same library after a reboot with no table on
//! disk; 52 bits, so the value is exact in a JSON double and in an `f64` anywhere it lands. Two
//! GUIDs in one library colliding needs ~2^26 items before it is even likely.
//!
//! The reverse direction needs memory: [`guid_of`] answers for every id this process has minted,
//! and every minted id came from a server answer, so a key the app holds has been seen — except a
//! key PERSISTED by an earlier run (a pin, a resume target). Those are libraries, which the
//! sections fetch re-mints at every boot, before anything can follow a pin.
//!
//! Track ids are the other trap: the app reads `sub_sid == 0` as "subtitles off" and any `id > 0`
//! as a track, while Jellyfin stream indexes start at 0 and `-1` means off. [`track_id`] /
//! [`stream_index`] are the only two places the off-by-one lives.
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

fn table() -> &'static Mutex<HashMap<i64, String>> {
    static T: OnceLock<Mutex<HashMap<i64, String>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `36EDF815-07B5-…` and `36edf81507b5…` are one id: lower-case, dashes and braces dropped.
pub fn normalize(guid: &str) -> String {
    guid.chars()
        .filter(|c| c.is_ascii_hexdigit())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The integer id for a GUID, minting the reverse entry. `0` for an empty or non-hex input — the
/// app's own "absent" — never a value a real GUID maps to.
pub fn intern(guid: &str) -> i64 {
    let norm = normalize(guid);
    if norm.len() < 13 {
        return 0;
    }
    let id = i64::from_str_radix(&norm[..13], 16).unwrap_or(0).max(1);
    if let Ok(mut t) = table().lock() {
        t.entry(id).or_insert(norm);
    }
    id
}

/// [`intern`] as the decimal `ratingKey` string, `""` for an absent GUID.
pub fn rating_key(guid: &str) -> String {
    match intern(guid) {
        0 => String::new(),
        id => id.to_string(),
    }
}

/// The GUID behind an id this process minted.
pub fn guid_of(id: i64) -> Option<String> {
    table().lock().ok()?.get(&id).cloned()
}

/// The GUID behind a decimal `ratingKey` (or section key) string.
pub fn guid_of_key(key: &str) -> Option<String> {
    guid_of(key.trim().parse().ok()?)
}

/// Jellyfin `MediaStream.Index` (0-based) → the app's track id (`> 0` is a track).
pub fn track_id(index: i64) -> i64 {
    if index < 0 { 0 } else { index + 1 }
}

/// The app's track id → Jellyfin `AudioStreamIndex`/`SubtitleStreamIndex`; `-1` for "off".
pub fn stream_index(track_id: i64) -> i64 {
    if track_id <= 0 { -1 } else { track_id - 1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mapping_is_a_pure_function_of_the_guid_and_survives_a_restart() {
        let a = intern("36edf81507b5c8eda566778b2a316c29");
        // what a second process computes, with no table
        assert_eq!(a, i64::from_str_radix("36edf81507b5c", 16).unwrap());
        assert!(a > 0 && (a as f64) as i64 == a, "exact in a double");
    }

    #[test]
    fn dashed_and_upper_case_spellings_are_the_same_item() {
        let plain = intern("36edf81507b5c8eda566778b2a316c29");
        assert_eq!(intern("36EDF815-07B5-C8ED-A566-778B2A316C29"), plain);
        assert_eq!(guid_of(plain).as_deref(), Some("36edf81507b5c8eda566778b2a316c29"));
        assert_eq!(guid_of_key(&plain.to_string()).as_deref(), Some("36edf81507b5c8eda566778b2a316c29"));
    }

    #[test]
    fn an_absent_guid_is_the_apps_absent_never_a_real_key() {
        assert_eq!(intern(""), 0);
        assert_eq!(rating_key(""), "");
        assert_eq!(intern("not-a-guid"), 0);
        assert_eq!(intern("0000000000000000000000000000000a"), 1, "zero prefix still a key");
    }

    #[test]
    fn track_ids_are_shifted_so_zero_stays_off() {
        assert_eq!(track_id(0), 1);
        assert_eq!(track_id(14), 15);
        assert_eq!(track_id(-1), 0);
        for idx in [0, 1, 52] {
            assert_eq!(stream_index(track_id(idx)), idx);
        }
        assert_eq!(stream_index(0), -1);
    }
}
