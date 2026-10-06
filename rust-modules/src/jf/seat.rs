//! **Which origins are Jellyfin servers**, and what this process has learned about each.
//!
//! The app reaches every server through one `plex::Client` per registry slot, and that client is
//! what every store, screen and worker already holds. A Jellyfin server is a `Client` too — its
//! token rides `Client::token`, its address `Client::origin` — and this table is the one bit that
//! makes the client speak Jellyfin instead of PMS: `Client::jf()` asks it.
//!
//! Keyed by ORIGIN (scheme + host + port), not machine id, because a dev boot installs a server
//! before anything has asked it who it is, and a Jellyfin server's `Id` is learned from the same
//! anonymous probe that classifies it. An origin is not shared between a PMS and a Jellyfin
//! server: one listening socket answers as one product.
//!
//! The user id is learned lazily from `GET /Users/Me` (the token names the user), so a seat needs
//! nothing at registration beyond the origin.
use crate::catalog::Origin;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Debug, Default)]
pub struct Seat {
    /// `UserDto.Id` of the signed-in user — `""` until `/Users/Me` has answered.
    pub user_id: String,
    /// `PublicSystemInfo.Id`, `""` until probed.
    pub server_id: String,
    pub server_name: String,
    pub server_version: String,
    /// The user's display name, as `/Users/Me` or the sign-in answered it.
    pub user_name: String,
    /// The name `DeviceId` is derived from, fixed for the life of the token: the username typed
    /// at a password sign-in, `""` for QuickConnect (the user is unknown when the device asks).
    /// Kept apart from `user_name` so learning the display name cannot re-key the device.
    pub device_user: String,
}

fn table() -> &'static Mutex<HashMap<String, Seat>> {
    static T: OnceLock<Mutex<HashMap<String, Seat>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(HashMap::new()))
}

fn key(origin: &Origin) -> String {
    origin.base().to_ascii_lowercase()
}

/// Mark `origin` as a Jellyfin server. Idempotent; keeps what was already learned.
pub fn register(origin: &Origin) {
    if let Ok(mut t) = table().lock() {
        t.entry(key(origin)).or_default();
    }
}

/// Mark `origin` as Jellyfin with what a sign-in already established.
pub fn register_with(origin: &Origin, seat: Seat) {
    if let Ok(mut t) = table().lock() {
        t.insert(key(origin), seat);
    }
}

pub fn forget(origin: &Origin) {
    if let Ok(mut t) = table().lock() {
        t.remove(&key(origin));
    }
}

pub fn is_jf(origin: &Origin) -> bool {
    table().lock().map(|t| t.contains_key(&key(origin))).unwrap_or(false)
}

pub fn get(origin: &Origin) -> Option<Seat> {
    table().lock().ok()?.get(&key(origin)).cloned()
}

/// Apply `f` to the seat in place; no-op for an origin that is not Jellyfin.
pub fn update(origin: &Origin, f: impl FnOnce(&mut Seat)) {
    if let Ok(mut t) = table().lock() {
        if let Some(s) = t.get_mut(&key(origin)) {
            f(s);
        }
    }
}

/// Is ANY Jellyfin seat registered — the UI's "this install speaks Jellyfin" question.
pub fn any() -> bool {
    table().lock().map(|t| !t.is_empty()).unwrap_or(false)
}

#[cfg(test)]
pub fn reset_for_test() {
    if let Ok(mut t) = table().lock() {
        t.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seat_is_keyed_by_the_whole_origin() {
        let _g = nj_base::testlock::serial();
        reset_for_test();
        let a = Origin::parse("http://10.0.0.2:8096").unwrap();
        let other_port = Origin::parse("http://10.0.0.2:32400").unwrap();
        let other_scheme = Origin::parse("https://10.0.0.2:8096").unwrap();
        register(&a);
        assert!(is_jf(&a));
        assert!(!is_jf(&other_port) && !is_jf(&other_scheme));
        update(&a, |s| s.user_id = "u1".into());
        register(&a);
        assert_eq!(get(&a).unwrap().user_id, "u1", "re-registering keeps what was learned");
        forget(&a);
        assert!(!is_jf(&a));
    }
}
