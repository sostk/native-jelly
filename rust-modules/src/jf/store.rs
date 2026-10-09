//! **The Jellyfin sign-in this install keeps**: which server, and the access token it minted for
//! this television. Kept inside the session record (`session::set_jellyfin_sign_in`), so on a
//! television it reaches the DB8 helper with the Plex credentials; read once at boot and replaced
//! on every sign-in. Its own file (`paths::jellyfin_candidates`, 0600, the session file's atomic
//! door) is the fallback for a record that cannot take it, and the one earlier builds wrote.
//!
//! The process also holds the live copy ([`current`]), because the account chip and the sign-out
//! path ask "who is signed in" every frame and must not read storage to answer it. Only [`load`]
//! and the memory writers ([`set_live`], [`remember`], [`activate`], [`forget_user`]) change it,
//! so a test that never calls them sees nobody signed in. The storage half ([`persist`],
//! [`erase`]) is IO and belongs on the storage worker.
//!
//! **More than one user of the same server** can be kept ([`Roster`]): every sign-in this
//! television has made, each with its own token and DeviceId basis, and which of them is
//! active. The record is written so that an OLDER build reading it still finds a sign-in: the
//! active user's own fields stay at the top level, exactly where a version-1 record kept them,
//! and the whole list rides beside them under `users` — a field an older build ignores. A
//! version-1 record reads back as a roster of one.
//!
//! **The server survives a sign-out** ([`RecentServer`]): its address, name and id — never a
//! token or a user — stay behind when the last user signs out, so the sign-in screen can offer
//! it as *Recent*. Only Delete all local data ([`forget_everything`] + [`erase`]) removes it. A
//! record holding nothing else carries no top-level sign-in fields, so no build reads it as one.
use super::auth::SignedIn;
use super::seat::Seat;
use crate::catalog::session::Session;
use crate::catalog::Origin;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

/// The record shape this build writes: version 2 is version 1's fields for the ACTIVE user plus
/// `users`, the whole roster.
const VERSION: u32 = 2;

/// At most this many users are kept. A sign-in past it replaces the user signed in longest ago.
pub const MAX_USERS: usize = 12;

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
    /// The profile key this user's pins, searches and offsets are filed under.
    pub fn profile_key(&self) -> String {
        crate::catalog::session::jellyfin_profile_key(&super::ids::normalize(&self.user_id))
    }

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

    /// Whether `self` and `other` are the same person on the same server — the roster's identity.
    /// The server is compared by origin AND by its own id when both know it, so one server reached
    /// at two addresses is still one server's user.
    pub fn same_user(&self, other: &Stored) -> bool {
        let ids = |a: &str, b: &str| super::ids::normalize(a) == super::ids::normalize(b);
        let same_server = self.server == other.server
            || (!self.server_id.is_empty() && !other.server_id.is_empty() && ids(&self.server_id, &other.server_id));
        same_server && !self.user_id.is_empty() && ids(&self.user_id, &other.user_id)
    }
}

/// **The server this television last signed in to**: what names it and reaches it, nothing that
/// signs anyone in. Kept after a sign-out for the sign-in screen's *Recent* row.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentServer {
    /// `Origin::base()` of the server.
    pub server: String,
    #[serde(default)]
    pub server_id: String,
    #[serde(default)]
    pub server_name: String,
}

impl RecentServer {
    fn of(s: &Stored) -> Self {
        Self { server: s.server.clone(), server_id: s.server_id.clone(), server_name: s.server_name.clone() }
    }

    pub fn origin(&self) -> Option<Origin> {
        Origin::parse(&self.server)
    }
}

/// **Every Jellyfin user this television keeps a sign-in for**, which one is active, and the
/// server signed in to last.
///
/// Ordered most recently signed in first; `active` indexes `users` and is `None` while nobody is
/// signed in (a roster of users kept for the who's-watching picker, with none of them chosen yet).
/// `recent` outlives the users: signing everyone out leaves it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Roster {
    pub users: Vec<Stored>,
    pub active: Option<usize>,
    pub recent: Option<RecentServer>,
}

impl Roster {
    /// The active user, if any.
    pub fn current(&self) -> Option<&Stored> {
        self.active.and_then(|i| self.users.get(i))
    }

    /// Keep `s` and make it the active user: it replaces the same user's older sign-in (a fresh
    /// token) and moves to the front. Past [`MAX_USERS`] the user at the back is dropped.
    pub fn remember(&mut self, s: Stored) {
        self.users.retain(|u| !u.same_user(&s));
        self.recent = Some(RecentServer::of(&s));
        self.users.insert(0, s);
        self.users.truncate(MAX_USERS);
        self.active = Some(0);
    }

    /// Make the kept user `user_id` (on `server`) the active one. `false` when no such user is kept.
    pub fn activate(&mut self, server: &str, user_id: &str) -> bool {
        let probe = Stored { server: server.to_owned(), user_id: user_id.to_owned(), ..Stored::default() };
        match self.users.iter().position(|u| u.same_user(&probe)) {
            Some(i) => {
                self.active = Some(i);
                true
            }
            None => false,
        }
    }

    /// Stop keeping the user `user_id` (on `server`). The active one leaves nobody active.
    pub fn forget(&mut self, server: &str, user_id: &str) -> bool {
        let probe = Stored { server: server.to_owned(), user_id: user_id.to_owned(), ..Stored::default() };
        let Some(i) = self.users.iter().position(|u| u.same_user(&probe)) else { return false };
        self.users.remove(i);
        self.active = match self.active {
            Some(a) if a == i => None,
            Some(a) if a > i => Some(a - 1),
            a => a,
        };
        true
    }

    /// Sign every user out and keep only the server signed in to last.
    pub fn sign_out_all(&mut self) {
        self.users.clear();
        self.active = None;
    }

    /// The record this roster is kept as: the active user's fields at the top (where a version-1
    /// reader looks), the whole list under `users`, the last server under `recent_server`. With no
    /// users it is `version` and `recent_server` alone — no field an older build reads as a
    /// sign-in. `None` when there is nothing at all to keep.
    fn to_value(&self) -> Option<serde_json::Value> {
        let recent = self.recent.as_ref().map(serde_json::to_value).transpose().ok()?;
        if self.users.is_empty() {
            return Some(serde_json::json!({ "version": VERSION, "recent_server": recent? }));
        }
        let top = self.current().cloned().unwrap_or_default();
        let mut value = serde_json::to_value(Stored { version: VERSION, ..top }).ok()?;
        let users: Vec<Stored> = self.users.iter().map(|u| Stored { version: VERSION, ..u.clone() }).collect();
        let object = value.as_object_mut()?;
        object.insert("users".into(), serde_json::to_value(users).ok()?);
        if let Some(recent) = recent {
            object.insert("recent_server".into(), recent);
        }
        Some(value)
    }

    /// Read a kept record of either version. A version-1 record is a roster of one, active; in a
    /// version-2 record the active user is the one whose fields sit at the top. Entries without a
    /// token or a server are not anyone and are dropped. A record without `recent_server` (one
    /// written before it existed) takes the active user's server, else the newest user's.
    fn from_value(value: &serde_json::Value) -> Option<Roster> {
        let top = serde_json::from_value::<Stored>(value.clone()).ok();
        let listed = value
            .get("users")
            .and_then(|u| serde_json::from_value::<Vec<Stored>>(u.clone()).ok())
            .unwrap_or_default();
        let mut users: Vec<Stored> = if listed.is_empty() { top.iter().cloned().collect() } else { listed };
        users.retain(Stored::usable);
        users.truncate(MAX_USERS);
        let active = top.filter(Stored::usable).and_then(|t| users.iter().position(|u| u.same_user(&t)));
        let recent = value
            .get("recent_server")
            .and_then(|r| serde_json::from_value::<RecentServer>(r.clone()).ok())
            .filter(|r| r.origin().is_some())
            .or_else(|| active.or(if users.is_empty() { None } else { Some(0) }).map(|i| RecentServer::of(&users[i])));
        if users.is_empty() && recent.is_none() {
            return None;
        }
        Some(Roster { users, active, recent })
    }
}

static ROSTER: Mutex<Roster> = Mutex::new(Roster { users: Vec::new(), active: None, recent: None });

/// Every change to the live roster goes through here, so the profile key the rest of the app
/// files history under (`session::current_profile_key`) always names the active user.
fn with_roster<T>(f: impl FnOnce(&mut Roster) -> T) -> T {
    let mut roster = ROSTER.lock().unwrap_or_else(|e| e.into_inner());
    let out = f(&mut roster);
    let active = roster.current().map(|u| super::ids::normalize(&u.user_id));
    crate::catalog::session::set_jellyfin_profile(active.as_deref());
    out
}

/// Make `s` the sign-in this process runs on, keeping it in the roster (`None`: nobody is signed
/// in and no user is kept; the server signed in to last stays, for *Recent*). Memory only.
pub fn set_live(s: Option<Stored>) {
    with_roster(|r| match s {
        Some(s) => r.remember(s),
        None => r.sign_out_all(),
    });
}

/// Forget every user AND the server signed in to last — Delete all local data. Memory only; the
/// caller removes the stored copies ([`erase`]).
pub fn forget_everything() {
    with_roster(|r| *r = Roster::default());
}

/// The server signed in to last, kept through a sign-out for the sign-in screen's *Recent* row.
pub fn recent_server() -> Option<RecentServer> {
    with_roster(|r| r.recent.clone())
}

/// Keep `s` beside the users already kept and make it the active one. Memory only.
pub fn remember(s: Stored) {
    with_roster(|r| r.remember(s));
}

/// Make the kept user `user_id` on `server` the active one. Memory only.
pub fn activate(server: &str, user_id: &str) -> bool {
    with_roster(|r| r.activate(server, user_id))
}

/// Stop keeping the user `user_id` on `server`. Memory only; [`persist`] writes the result.
pub fn forget_user(server: &str, user_id: &str) -> bool {
    with_roster(|r| r.forget(server, user_id))
}

/// Every user kept, most recently signed in first, and which one is active.
pub fn roster() -> Roster {
    with_roster(|r| r.clone())
}

/// The sign-in this process is running on, if it is a Jellyfin one.
pub fn current() -> Option<Stored> {
    with_roster(|r| r.current().cloned())
}

/// The name the account chip shows while a Jellyfin sign-in is live.
pub fn signed_in_name() -> Option<String> {
    current().map(|s| if s.user_name.is_empty() { s.server_name } else { s.user_name })
}

fn candidates() -> Vec<PathBuf> {
    nj_base::paths::jellyfin_candidates()
}

/// Read the kept roster — the one `session` (the record boot just loaded) carries, else the first
/// file candidate holding a usable one — and make it the live one. Answers the active user.
pub fn load(session: &Session) -> Option<Stored> {
    let roster = from_session(session).or_else(|| load_from(&candidates())).unwrap_or_default();
    let active = roster.current().cloned();
    with_roster(|r| *r = roster);
    active
}

/// Keep `roster` in the session record, else in the first file candidate that takes it; a roster
/// with nothing to keep, not even a recent server, removes every copy ([`erase`]). `false` when neither held it: the sign-ins still hold
/// for this run, they just will not survive a restart.
pub fn persist(roster: &Roster) -> bool {
    let Some(value) = roster.to_value() else {
        erase();
        return true;
    };
    let in_record = crate::catalog::session::set_jellyfin_sign_in(Some(value.clone()));
    let ok = if in_record {
        // A file copy left behind would be one more token at rest, and stale at the next sign-in.
        forget_at(&candidates());
        true
    } else {
        nj_base::eventlog::log("jf: the session record did not take the sign-in — trying its own file");
        save_to(&candidates(), &value)
    };
    if !ok {
        nj_base::eventlog::log("jf: the sign-in could not be saved — it lasts until the app closes");
    }
    ok
}

/// Remove every stored copy.
pub fn erase() {
    let _ = crate::catalog::session::set_jellyfin_sign_in(None);
    forget_at(&candidates());
}

fn from_session(session: &Session) -> Option<Roster> {
    Roster::from_value(crate::catalog::session::jellyfin_sign_in(session)?)
}

fn load_from(paths: &[PathBuf]) -> Option<Roster> {
    paths.iter().find_map(|p| {
        let bytes = std::fs::read(p).ok()?;
        Roster::from_value(&serde_json::from_slice::<serde_json::Value>(&bytes).ok()?)
    })
}

fn save_to(paths: &[PathBuf], value: &serde_json::Value) -> bool {
    let Ok(json) = serde_json::to_vec_pretty(value) else { return false };
    for (i, p) in paths.iter().enumerate() {
        if crate::catalog::session::write_atomic(p, &json).is_ok() {
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
        let mut roster = Roster::default();
        roster.remember(s.clone());
        assert!(save_to(&paths, &roster.to_value().unwrap()));
        let back = load_from(&paths).unwrap();
        assert_eq!(back, roster);
        let back = back.current().unwrap().clone();
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
        let mut fresh = Roster::default();
        fresh.remember(Stored::new(&origin, &signed_in()));
        assert!(save_to(&paths, &fresh.to_value().unwrap()));
        assert_eq!(load_from(&paths).unwrap().current().unwrap().token, "tok");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_sign_in_kept_in_the_session_record_survives_a_reload_and_goes_with_erase() {
        use crate::catalog::session;
        let _g = nj_base::testlock::serial();
        let t = session::TempSession::new("jf-store");
        let s = Stored::new(&Origin::parse("http://10.0.0.2:8096").unwrap(), &signed_in());
        assert!(session::set_jellyfin_sign_in(Some(serde_json::to_value(&s).unwrap())));
        let on_disk: serde_json::Value = serde_json::from_slice(&std::fs::read(t.path()).unwrap()).unwrap();
        assert_eq!(on_disk["client_id"], "cid-test", "the rest of the record is kept");
        assert_eq!(on_disk["jellyfin"]["token"], "tok");
        assert_eq!(from_session(&session::load()).and_then(|r| r.current().cloned()), Some(s.clone()));
        assert!(session::set_jellyfin_sign_in(Some(serde_json::to_value(&s).unwrap())),
            "the same sign-in again is already held");
        assert!(session::set_jellyfin_sign_in(None));
        assert!(from_session(&session::load()).is_none());
        let on_disk: serde_json::Value = serde_json::from_slice(&std::fs::read(t.path()).unwrap()).unwrap();
        assert!(on_disk.get("jellyfin").is_none());
    }

    #[test]
    fn a_record_with_nothing_readable_does_not_take_a_sign_in() {
        use crate::catalog::session;
        let _g = nj_base::testlock::serial();
        let _t = session::TempSession::new("jf-store-empty");
        session::save(&session::Session::default());
        let s = Stored::new(&Origin::parse("http://10.0.0.2:8096").unwrap(), &signed_in());
        assert!(!session::set_jellyfin_sign_in(Some(serde_json::to_value(&s).unwrap())),
            "without a client_id the record may be a session that failed to read");
    }

    fn user(id: &str, token: &str) -> Stored {
        Stored {
            token: token.into(),
            user_id: id.into(),
            user_name: id.to_uppercase(),
            device_user: id.into(),
            ..Stored::new(&Origin::parse("http://10.0.0.2:8096").unwrap(), &signed_in())
        }
    }

    /// A record an earlier build wrote — one sign-in, no `users` — is a roster of one, active.
    #[test]
    fn a_version_one_record_is_a_roster_of_one() {
        let v1 = serde_json::json!({"version":1,"server":"http://10.0.0.2:8096","token":"tok","user_id":"u1",
            "user_name":"Cursor","server_id":"AB-CD","server_name":"Home","server_version":"12.0.0","device_user":"cursor"});
        let r = Roster::from_value(&v1).unwrap();
        assert_eq!(r.users.len(), 1);
        assert_eq!(r.current().unwrap().token, "tok");
    }

    /// The record keeps the ACTIVE user's sign-in where a version-1 reader looks, so going back to
    /// an older build keeps that user signed in instead of signing everyone out.
    #[test]
    fn an_older_build_reading_the_record_finds_the_active_user() {
        let mut r = Roster::default();
        r.remember(user("alex", "t-alex"));
        r.remember(user("sam", "t-sam"));
        assert!(r.activate("http://10.0.0.2:8096", "alex"));
        let value = r.to_value().unwrap();
        let as_v1: Stored = serde_json::from_value(value.clone()).unwrap();
        assert_eq!((as_v1.user_id.as_str(), as_v1.token.as_str()), ("alex", "t-alex"));
        assert!(as_v1.usable());
        assert_eq!(Roster::from_value(&value).unwrap(), r, "and this build reads the whole roster back");
    }

    /// Signing a user in again refreshes their token in place and brings them to the front; a
    /// different user joins the list; past the limit the oldest one goes.
    #[test]
    fn remembering_replaces_the_same_user_and_keeps_the_others() {
        let mut r = Roster::default();
        r.remember(user("alex", "t1"));
        r.remember(user("sam", "t2"));
        r.remember(user("alex", "t3"));
        let order: Vec<(&str, &str)> = r.users.iter().map(|u| (u.user_id.as_str(), u.token.as_str())).collect();
        assert_eq!(order, [("alex", "t3"), ("sam", "t2")]);
        assert_eq!(r.current().unwrap().user_id, "alex");
        for i in 0..MAX_USERS + 3 {
            r.remember(user(&format!("u{i}"), "t"));
        }
        assert_eq!(r.users.len(), MAX_USERS);
        assert!(r.users.iter().all(|u| u.user_id != "sam"), "the user signed in longest ago went");
    }

    /// Switching makes another kept user active; forgetting the active one leaves nobody active
    /// and the rest kept; an unknown user changes nothing.
    #[test]
    fn activating_and_forgetting_move_the_active_user() {
        let mut r = Roster::default();
        r.remember(user("alex", "ta"));
        r.remember(user("sam", "ts"));
        assert!(r.activate("http://10.0.0.2:8096", "alex"));
        assert_eq!(r.current().unwrap().user_id, "alex");
        assert!(!r.activate("http://10.0.0.2:8096", "nobody"));
        assert!(!r.activate("http://10.9.9.9:8096", "sam"), "another server's user is not this one");
        assert!(r.forget("http://10.0.0.2:8096", "alex"));
        assert!(r.current().is_none());
        assert_eq!(r.users.len(), 1);
        assert!(r.activate("http://10.0.0.2:8096", "sam"));
        assert!(r.forget("http://10.0.0.2:8096", "sam"));
        let value = r.to_value().unwrap();
        assert!(value.get("users").is_none() && value.get("token").is_none(), "no user is kept: {value}");
        assert_eq!(value["recent_server"]["server"], "http://10.0.0.2:8096", "only the server is");
        assert!(Roster::default().to_value().is_none(), "an empty roster keeps nothing");
    }

    /// A kept user with no token is not anyone, in either version of the record.
    #[test]
    fn a_roster_drops_users_without_a_token() {
        let value = serde_json::json!({"version":2,"server":"http://10.0.0.2:8096","token":"ta","user_id":"alex",
            "users":[{"server":"http://10.0.0.2:8096","token":"ta","user_id":"alex"},
                     {"server":"http://10.0.0.2:8096","token":"","user_id":"sam"}]});
        let r = Roster::from_value(&value).unwrap();
        assert_eq!(r.users.len(), 1);
        assert_eq!(r.current().unwrap().user_id, "alex");
    }

    /// The live roster: a sign-in is kept beside the others, `set_live(None)` (today's sign-out)
    /// still forgets every user — but not the server, which only `forget_everything` drops.
    #[test]
    fn the_live_roster_keeps_every_sign_in_until_a_sign_out() {
        let _g = nj_base::testlock::serial();
        forget_everything();
        assert!(recent_server().is_none());
        set_live(Some(user("alex", "ta")));
        remember(user("sam", "ts"));
        assert_eq!(current().unwrap().user_id, "sam");
        assert_eq!(roster().users.len(), 2);
        assert!(activate("http://10.0.0.2:8096", "alex"));
        assert_eq!(current().unwrap().user_id, "alex");
        assert!(forget_user("http://10.0.0.2:8096", "sam"));
        assert_eq!(roster().users.len(), 1);
        set_live(None);
        assert!(current().is_none() && roster().users.is_empty());
        assert_eq!(recent_server().unwrap().server, "http://10.0.0.2:8096");
        forget_everything();
        assert!(recent_server().is_none());
    }

    /// After everyone signs out the record keeps the server and nothing that signs anyone in: no
    /// token, no user, and no top-level field an older build would read as a sign-in.
    #[test]
    fn signing_everyone_out_keeps_only_the_server() {
        let mut r = Roster::default();
        r.remember(user("alex", "ta"));
        r.remember(user("sam", "ts"));
        r.sign_out_all();
        let value = r.to_value().unwrap();
        let text = value.to_string();
        assert!(!text.contains("\"ta\"") && !text.contains("\"ts\"") && !text.contains("alex"), "{text}");
        assert!(serde_json::from_value::<Stored>(value.clone()).is_err(), "an older build sees no sign-in");
        let back = Roster::from_value(&value).unwrap();
        assert!(back.users.is_empty() && back.current().is_none());
        assert_eq!(back.recent, Some(RecentServer {
            server: "http://10.0.0.2:8096".into(), server_id: "AB-CD".into(), server_name: "Home".into() }));
    }

    /// A record written before `recent_server` existed still offers its server as *Recent*.
    #[test]
    fn an_earlier_record_offers_its_server_as_recent() {
        let v1 = serde_json::json!({"version":1,"server":"http://10.0.0.2:8096","token":"tok","user_id":"u1",
            "server_name":"Home"});
        let r = Roster::from_value(&v1).unwrap();
        assert_eq!(r.recent.unwrap().server_name, "Home");
    }

    /// The active Jellyfin user is the profile every per-user list is filed under: switching
    /// moves the key, signing everyone out clears it.
    #[test]
    fn each_jellyfin_user_files_history_under_their_own_key() {
        use crate::catalog::session::current_profile_key;
        let _g = nj_base::testlock::serial();
        let (alex, sam) = (user("alex", "ta"), user("sam", "ts"));
        assert_ne!(alex.profile_key(), sam.profile_key());
        set_live(Some(alex.clone()));
        assert_eq!(current_profile_key(), alex.profile_key());
        remember(sam.clone());
        assert_eq!(current_profile_key(), sam.profile_key());
        assert!(activate("http://10.0.0.2:8096", "alex"));
        assert_eq!(current_profile_key(), alex.profile_key());
        set_live(None);
        let after = current_profile_key();
        assert!(after != alex.profile_key() && after != sam.profile_key(), "nobody is active: {after}");
        forget_everything();
    }

    /// An install upgraded from one Jellyfin user kept that user's history under the empty key; it
    /// becomes the active user's, list by list, and never over a list that user already has.
    #[test]
    fn earlier_history_becomes_the_active_users_and_never_overwrites_theirs() {
        use crate::catalog::session::{adopt_unscoped_profile, HomePins, LastLibrary, RecentSearches, Session};
        let key = user("alex", "ta").profile_key();
        let s = Session {
            home_pins: vec![HomePins { user: String::new(), asked: true, ..Default::default() }],
            recent_searches: vec![
                RecentSearches { user: String::new(), terms: vec!["old".into()], ..Default::default() },
                RecentSearches { user: key.clone(), terms: vec!["mine".into()], ..Default::default() },
            ],
            last_library: vec![LastLibrary { user: "jf-someone-else".into(), ..Default::default() }],
            ..Default::default()
        };
        let next = adopt_unscoped_profile(&s, &key).expect("the pins move");
        assert_eq!(next.home_pins[0].user, key);
        assert!(next.pins_for(&key).is_some_and(|p| p.asked), "the answer moved with the pins");
        let mine: Vec<_> = next.recent_searches.iter().filter(|r| r.user == key).collect();
        assert_eq!(mine.len(), 1, "their own searches stay theirs alone");
        assert_eq!(mine[0].terms, ["mine"]);
        assert_eq!(next.last_library[0].user, "jf-someone-else");
        assert!(adopt_unscoped_profile(&next, &key).is_none(), "nothing left to move");
        assert!(adopt_unscoped_profile(&s, "").is_none());
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
