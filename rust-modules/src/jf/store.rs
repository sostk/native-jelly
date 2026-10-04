//! **The Jellyfin sign-in this install keeps**: which server, and the access token it minted for
//! this television. One file (`paths::jellyfin_candidates`), written 0600 through the same atomic
//! door as the session file, read once at boot and replaced on every sign-in.
//!
//! The process also holds the live copy ([`current`]), because the account chip and the sign-out
//! path ask "who is signed in" every frame and must not read a file to answer it. Only [`load`]
//! and [`set_live`] change it, so a test that never calls them sees nobody signed in. The file
//! half ([`persist`], [`erase`]) is IO and belongs on the storage worker.
use super::auth::SignedIn;
use super::seat::Seat;
use crate::plex::Origin;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

const VERSION: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stored {
    #[serde(default)]
    pub version: u32,
    /// `Origin::base()` of the server — scheme, host and port, never a path.
    pub server: String,
    pub token: String,
    pub user_id: String,
    #[serde(default)]
    pub user_name: String,
    #[serde(default)]
    pub server_id: String,
    #[serde(default)]
    pub server_name: String,
    #[serde(default)]
    pub server_version: String,
    /// The DeviceId basis the token was minted under ([`Seat::device_user`]). Restored verbatim:
    /// a different basis would present the token from a device the server never issued it to.
    #[serde(default)]
    pub device_user: String,
}

impl Stored {
    pub fn new(origin: &Origin, s: &SignedIn) -> Self {
        Self {
            version: VERSION,
            server: origin.base(),
            token: s.token.clone(),
            user_id: s.user_id.clone(),
            user_name: s.user_name.clone(),
            server_id: s.server_id.clone(),
            server_name: s.server_name.clone(),
            server_version: s.server_version.clone(),
            device_user: s.device_user.clone(),
        }
    }

    pub fn origin(&self) -> Option<Origin> {
        Origin::parse(&self.server)
    }

    pub fn seat(&self) -> Seat {
        Seat {
            user_id: self.user_id.clone(),
            server_id: super::ids::normalize(&self.server_id),
            server_name: self.server_name.clone(),
            server_version: self.server_version.clone(),
            user_name: self.user_name.clone(),
            device_user: self.device_user.clone(),
        }
    }

    fn usable(&self) -> bool {
        !self.token.is_empty() && self.origin().is_some()
    }
}

static CURRENT: Mutex<Option<Stored>> = Mutex::new(None);

/// Make `s` the sign-in this process runs on (`None`: signed out). Memory only.
pub fn set_live(s: Option<Stored>) {
    *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = s;
}

/// The sign-in this process is running on, if it is a Jellyfin one.
pub fn current() -> Option<Stored> {
    CURRENT.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The name the account chip shows while a Jellyfin sign-in is live.
pub fn signed_in_name() -> Option<String> {
    current().map(|s| if s.user_name.is_empty() { s.server_name } else { s.user_name })
}

fn candidates() -> Vec<PathBuf> {
    plx_base::paths::jellyfin_candidates()
}

/// Read the stored sign-in (the first candidate that holds a usable one) and make it [`current`].
pub fn load() -> Option<Stored> {
    let s = load_from(&candidates());
    set_live(s.clone());
    s
}

/// Write `s` to the first candidate that takes it. `false` when none did: the sign-in still holds
/// for this run, it just will not survive a restart.
pub fn persist(s: &Stored) -> bool {
    let ok = save_to(&candidates(), s);
    if !ok {
        plx_base::eventlog::log("jf: the sign-in could not be saved — it lasts until the app closes");
    }
    ok
}

/// Remove every stored copy.
pub fn erase() {
    forget_at(&candidates());
}

fn load_from(paths: &[PathBuf]) -> Option<Stored> {
    paths.iter().find_map(|p| {
        let bytes = std::fs::read(p).ok()?;
        serde_json::from_slice::<Stored>(&bytes).ok().filter(Stored::usable)
    })
}

fn save_to(paths: &[PathBuf], s: &Stored) -> bool {
    let Ok(json) = serde_json::to_vec_pretty(s) else { return false };
    for (i, p) in paths.iter().enumerate() {
        if crate::plex::session::write_atomic(p, &json).is_ok() {
            // A copy left at a better candidate would win the next `load` over this one.
            forget_at(&paths[..i]);
            return true;
        }
    }
    false
}

fn forget_at(paths: &[PathBuf]) {
    for p in paths {
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signed_in() -> SignedIn {
        SignedIn {
            token: "tok".into(),
            user_id: "u1".into(),
            user_name: "Cursor".into(),
            server_id: "AB-CD".into(),
            server_name: "Home".into(),
            server_version: "12.0.0".into(),
            device_user: "cursor".into(),
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nj-store-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_saved_sign_in_reads_back_whole_and_private() {
        use std::os::unix::fs::PermissionsExt;
        let d = temp_dir("roundtrip");
        let origin = Origin::parse("http://10.0.0.2:8096").unwrap();
        let s = Stored::new(&origin, &signed_in());
        let paths = [d.join("jellyfin.json")];
        assert!(save_to(&paths, &s));
        let back = load_from(&paths).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.origin().unwrap().base(), "http://10.0.0.2:8096");
        assert_eq!(back.seat().server_id, "abcd");
        let mode = std::fs::metadata(&paths[0]).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "a token at rest is private");
        forget_at(&paths);
        assert!(load_from(&paths).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_save_skips_a_candidate_it_cannot_write_and_replaces_the_stale_copy() {
        let d = temp_dir("fallback");
        let origin = Origin::parse("https://jf.example:443").unwrap();
        let stale = Stored { token: "old".into(), ..Stored::new(&origin, &signed_in()) };
        let first = d.join("first.json");
        std::fs::write(&first, serde_json::to_vec(&stale).unwrap()).unwrap();
        let paths = [d.join("missing-dir").join("x.json"), first.clone(), d.join("second.json")];
        let fresh = Stored::new(&origin, &signed_in());
        assert!(save_to(&paths, &fresh));
        assert_eq!(load_from(&paths).unwrap().token, "tok");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_file_without_a_token_or_server_is_nobody() {
        let d = temp_dir("unusable");
        let p = d.join("jellyfin.json");
        std::fs::write(&p, br#"{"server":"http://10.0.0.2:8096","token":"","user_id":"u"}"#).unwrap();
        assert!(load_from(std::slice::from_ref(&p)).is_none());
        std::fs::write(&p, br#"{"server":"","token":"t","user_id":"u"}"#).unwrap();
        assert!(load_from(std::slice::from_ref(&p)).is_none());
        std::fs::write(&p, b"not json").unwrap();
        assert!(load_from(std::slice::from_ref(&p)).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }
}
