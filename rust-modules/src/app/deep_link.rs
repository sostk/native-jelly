//! **Launch-parameter deep link: a launcher that opens Native Jelly on one item, as one user.**
//!
//! webOS hands a native app the `params` of a `luna://com.webos.applicationManager/launch` call as
//! one JSON string in `argv[1]` (`src/main.c` passes it to [`nj_launch_params`]); on webOS 6 and
//! later that string is SAM's lifecycle message, with the params under `parameters`. A launcher shelf
//! such as CleanYourTV's *Jellyfin continue watching* card sends `{"itemId": "<GUID>", "serverId":
//! "<GUID>"}` plus keys this app does not read (`tmdbId`, `mediaType`, `title`). A program that
//! wants a person's own library adds `"userId": "<GUID>"`.
//!
//! * `userId` **picks one of the users already kept on this television** — exactly the tile the
//!   who's-watching screen offers, so it needs nothing the remote could not do (Jellyfin users
//!   have no PIN here). It never signs anyone in, never takes a password or token, and a user that
//!   is not kept is dropped rather than guessed at. The picker is skipped; the person lands on
//!   Home (or the item).
//! * `itemId` opens that item's **detail page**, never playback. With both keys the item opens
//!   as that user.
//! * The link waits on Home — behind a sign-in, the picker or a first-run question the person
//!   still has to answer — and drops after [`WAIT_MS`], so a link that arrived while someone was
//!   typing a password does not yank them elsewhere a minute later. A `serverId` naming another
//!   server than the user's (or the signed-in one) is dropped for the same reason.
//!
//! Every id crosses a trust boundary (any app can call `launch`), so each is accepted only as a
//! GUID spelling — hex digits and dashes — and nothing else reaches the catalog or the log.
use super::bridge::{AppHost, Bridge};
use crate::screens::registry::AppArg;
use crate::ui::dispatch::Dispatcher;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// How long a link waits for Home before it is dropped.
const WAIT_MS: u32 = 45_000;
/// A launch parameter longer than this is not one of ours.
const MAX_PARAMS: usize = 2048;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DeepLink {
    /// Normalized (lower-case, undashed) Jellyfin item id to open.
    pub(crate) item: Option<String>,
    /// Normalized id of the kept user to be, when the launcher named one.
    pub(crate) user: Option<String>,
    /// Normalized id of the server the launcher read the item from, when it said.
    pub(crate) server: Option<String>,
}

/// A link armed at boot. `deadline` is the loop tick past which it is dropped, set on the first
/// frame because boot has no loop clock yet.
#[derive(Clone, Debug)]
pub(crate) struct Pending {
    link: DeepLink,
    deadline: Option<u32>,
    /// The who's-watching screen was asked for, to switch to `user` from another page.
    seated: bool,
    /// `user` was picked: only their Home is waited for now.
    picked: bool,
}

static LAUNCH: Mutex<Option<DeepLink>> = Mutex::new(None);
/// SAM relaunched the running app; the loop raises the window and arms any link in [`LAUNCH`].
static RELAUNCHED: AtomicBool = AtomicBool::new(false);

/// A Jellyfin GUID in any of its spellings, normalized; `None` for anything else.
fn guid(s: &str) -> Option<String> {
    if !s.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
        return None;
    }
    let n = crate::jf::ids::normalize(s);
    (n.len() == 32).then_some(n)
}

/// An optional GUID key: absent or null is `None`, a present non-GUID refuses the whole link.
fn opt_guid(v: &serde_json::Value, key: &str) -> Result<Option<String>, ()> {
    match v.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(s) => s.as_str().and_then(guid).map(Some).ok_or(()),
    }
}

/// The link in a launch-parameter string, or `None` when it names neither an item nor a user.
pub(crate) fn parse(params: &str) -> Option<DeepLink> {
    if params.len() > MAX_PARAMS {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(params).ok()?;
    // A native app on interface version 2 (webOS 6 and later) gets SAM's lifecycle message in
    // argv[1], with the launch params under `parameters` (measured on a C1, webOS 6.5).
    let v = match v.get("parameters") {
        Some(p) if p.is_object() => p,
        _ => &v,
    };
    let item = opt_guid(v, "itemId").ok()?;
    let user = opt_guid(v, "userId").ok()?;
    let server = opt_guid(v, "serverId").ok()?;
    (item.is_some() || user.is_some()).then_some(DeepLink { item, user, server })
}

/// The C shim's hand-over of `argv[1]`. Anything that is not a deep link is ignored: the sole
/// other launch argument is Sentry's envelope path, which `main.c` has already claimed.
#[no_mangle]
pub extern "C" fn nj_launch_params(params: *const std::os::raw::c_char) {
    if params.is_null() {
        return;
    }
    // SAFETY: `main.c` passes a NUL-terminated `argv` entry that outlives this call.
    stash(&unsafe { std::ffi::CStr::from_ptr(params) }.to_string_lossy());
}

fn stash(params: &str) {
    if let Some(link) = parse(params) {
        nj_base::eventlog::log("deep link: launched with a link");
        if let Ok(mut g) = LAUNCH.lock() {
            *g = Some(link);
        }
    }
}

/// What `main.c` should do about one of SAM's lifecycle messages.
#[derive(Debug, PartialEq, Eq)]
enum Lifecycle {
    Ignore,
    Relaunch,
    Close,
}

fn lifecycle(payload: &str) -> Lifecycle {
    let v: serde_json::Value = match serde_json::from_str(payload) {
        Ok(v) => v,
        Err(_) => return Lifecycle::Ignore,
    };
    if v.get("returnValue") == Some(&serde_json::Value::Bool(false)) {
        nj_base::eventlog::log("lifecycle: registerApp refused");
    }
    match v.get("event").and_then(|e| e.as_str()) {
        Some("relaunch") => Lifecycle::Relaunch,
        Some("close") => Lifecycle::Close,
        _ => Lifecycle::Ignore,
    }
}

/// SAM's `registerApp` subscription, which `main.c` takes over from LG's SDL because SDL drops a
/// relaunch's `parameters`. Called on libhelpers' thread; returns 2 for `close` (main.c quits).
#[no_mangle]
pub extern "C" fn nj_lifecycle(payload: *const std::os::raw::c_char) -> std::os::raw::c_int {
    if payload.is_null() {
        return 0;
    }
    // SAFETY: libhelpers passes the message's NUL-terminated payload, valid for this call.
    let s = unsafe { std::ffi::CStr::from_ptr(payload) }.to_string_lossy();
    match lifecycle(&s) {
        Lifecycle::Relaunch => {
            nj_base::eventlog::log("lifecycle: relaunch");
            stash(&s);
            RELAUNCHED.store(true, Ordering::Release);
            1
        }
        Lifecycle::Close => {
            nj_base::eventlog::log("lifecycle: close");
            2
        }
        Lifecycle::Ignore => 0,
    }
}

/// Whether SAM relaunched the app since the last call (once per relaunch).
pub(crate) fn take_relaunch() -> bool {
    RELAUNCHED.swap(false, Ordering::Acquire)
}

/// The link this process was launched with, once (the boot arms it into `App`).
pub(crate) fn take_launch() -> Option<Pending> {
    let link = LAUNCH.lock().ok()?.take()?;
    Some(Pending { link, deadline: None, seated: false, picked: false })
}

/// What one frame should do about an armed link.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    Wait,
    Drop(&'static str),
    /// Ask for the who's-watching screen, to pick from it.
    Seat,
    /// Pick the kept user at this roster position.
    Pick(usize),
    /// Only a user was named, and they are in.
    Done,
    Open,
}

/// What the loop sees this frame.
struct Ctx<'a> {
    /// Home is the settled top page and owns input.
    home: bool,
    /// The who's-watching screen is the settled top page and owns input.
    picker: bool,
    /// Every kept user: (user id, server id).
    users: &'a [(String, String)],
    active: Option<usize>,
}

fn step(p: &Pending, deadline: u32, now: u32, c: &Ctx) -> Step {
    if now.wrapping_sub(deadline) < u32::MAX / 2 {
        return Step::Drop("never reached Home");
    }
    let server_ok = |sid: &str| {
        p.link.server.as_ref().map_or(true, |want| sid.is_empty() || crate::jf::ids::normalize(sid) == *want)
    };
    if let Some(want) = &p.link.user {
        let Some(i) = c.users.iter().position(|(uid, sid)| crate::jf::ids::normalize(uid) == *want && server_ok(sid))
        else {
            return Step::Drop("that user is not kept on this television");
        };
        let me = c.active == Some(i);
        if !p.picked {
            if c.picker {
                return Step::Pick(i);
            }
            if !(me && c.home) {
                return if c.home && !p.seated { Step::Seat } else { Step::Wait };
            }
        } else if !(me && c.home) {
            return Step::Wait;
        }
    } else if !c.home {
        return Step::Wait;
    }
    if p.link.item.is_none() {
        return Step::Done;
    }
    match c.active.and_then(|i| c.users.get(i)) {
        Some((_, sid)) if !server_ok(sid) => Step::Drop("another server is signed in"),
        _ => Step::Open,
    }
}

/// Run every frame beside `menu_play_tick`: switch to the named user, then open the item's detail
/// page once their Home is up.
pub(crate) fn tick(slot: &mut Option<Pending>, pages: &mut Dispatcher<AppHost>, bridge: &mut Bridge, now: u32) {
    let Some(p) = slot.as_mut() else { return };
    let deadline = *p.deadline.get_or_insert(now.wrapping_add(WAIT_MS));
    let owns_input = pages
        .nav
        .top_page()
        .is_some_and(|e| pages.nav.input_owner() == Some(nj_machine::machine::InputOwner::Entry(e.id)));
    let roster = crate::jf::store::roster();
    let users: Vec<(String, String)> = roster.users.iter().map(|u| (u.user_id.clone(), u.server_id.clone())).collect();
    let ctx = Ctx {
        home: owns_input && matches!(pages.top_arg(), Some(AppArg::Home)) && !users.is_empty(),
        picker: owns_input && super::bridge::top_settled_on(pages, &AppArg::Profiles),
        users: &users,
        active: roster.active,
    };
    match step(p, deadline, now, &ctx) {
        Step::Wait => {}
        Step::Drop(why) => {
            nj_base::eventlog::log(&format!("deep link: dropped ({why})"));
            *slot = None;
        }
        Step::Seat => {
            p.seated = true;
            nj_base::eventlog::log("deep link: asking who's watching to switch user");
            super::bridge::switch_profile(pages);
        }
        Step::Pick(i) => {
            p.picked = true;
            nj_base::eventlog::log("deep link: picking the named user");
            super::jf_login::pick_user(pages, bridge, i);
        }
        Step::Done => {
            nj_base::eventlog::log("deep link: user is in");
            *slot = None;
        }
        Step::Open => {
            let rk = p.link.item.as_deref().map(crate::jf::ids::rating_key).unwrap_or_default();
            *slot = None;
            nj_base::eventlog::log("deep link: opening the item's detail page");
            super::bridge::open_detail(pages, bridge, crate::catalog::current_server(), &rk, None, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "36edf81507b5c8eda566778b2a316c29";
    const B: &str = "0123456789abcdef0123456789abcdef";
    const U1: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const U2: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn link(item: bool, user: Option<&str>, server: Option<&str>) -> Pending {
        Pending {
            link: DeepLink { item: item.then(|| A.into()), user: user.map(Into::into), server: server.map(Into::into) },
            deadline: Some(1000),
            seated: false,
            picked: false,
        }
    }

    fn step_in(p: &Pending, now: u32, home: bool, picker: bool, active: Option<usize>) -> Step {
        let users = [(U1.to_string(), B.to_string()), (U2.to_string(), B.to_string())];
        step(p, 1000, now, &Ctx { home, picker, users: &users, active })
    }

    #[test]
    fn parses_every_guid_spelling_and_ignores_the_launcher_extras() {
        let dashed = "36EDF815-07B5-C8ED-A566-778B2A316C29";
        let json = format!(r#"{{"itemId":"{dashed}","serverId":"{B}","userId":"{U1}","tmdbId":603,"title":"x"}}"#);
        let want = DeepLink { item: Some(A.into()), user: Some(U1.into()), server: Some(B.into()) };
        assert_eq!(parse(&json), Some(want));
        assert_eq!(
            parse(&format!(r#"{{"itemId":"{A}"}}"#)),
            Some(DeepLink { item: Some(A.into()), user: None, server: None })
        );
        assert_eq!(
            parse(&format!(r#"{{"userId":"{U1}","itemId":null}}"#)),
            Some(DeepLink { item: None, user: Some(U1.into()), server: None }),
            "a user alone is a link"
        );
    }

    #[test]
    fn reads_the_params_inside_sams_lifecycle_message() {
        // argv[1] of a launch on a webOS 6.5 C1 (nativeLifeCycleInterfaceVersion 2).
        let argv = format!(
            r#"{{"event":"launch","reason":"undefined","appId":"com.sostk.nativejelly","interfaceVersion":2,"parameters":{{"itemId":"{A}","userId":"{U1}"}},"interfaceMethod":"registerApp","@system_native_app":true}}"#
        );
        assert_eq!(parse(&argv), Some(DeepLink { item: Some(A.into()), user: Some(U1.into()), server: None }));
        assert_eq!(parse(r#"{"event":"launch","parameters":{}}"#), None, "a plain launch is no link");
    }

    #[test]
    fn sorts_sams_lifecycle_messages() {
        let relaunch = format!(r#"{{"event":"relaunch","appId":"x","parameters":{{"itemId":"{A}"}}}}"#);
        assert_eq!(lifecycle(&relaunch), Lifecycle::Relaunch);
        assert_eq!(lifecycle(r#"{"event":"close","reason":"memoryReclaim"}"#), Lifecycle::Close);
        for other in [r#"{"returnValue":true,"event":"registered"}"#, r#"{"returnValue":false}"#, "x", ""] {
            assert_eq!(lifecycle(other), Lifecycle::Ignore, "{other}");
        }
    }

    #[test]
    fn refuses_anything_that_is_not_a_guid_or_not_json() {
        for bad in [
            "", "not json", "{}", r#"{"serverId":"0123456789abcdef0123456789abcdef"}"#, r#"{"itemId":42}"#,
            r#"{"itemId":""}"#, r#"{"itemId":"abc"}"#, r#"{"itemId":"../../etc/passwd"}"#,
            r#"{"itemId":"36edf81507b5c8eda566778b2a316c2z"}"#, r#"{"userId":"nope"}"#, r#"{"userId":7}"#,
            r#"{"itemId":"36edf81507b5c8eda566778b2a316c29","userId":"x"}"#,
            r#"{"itemId":"36edf81507b5c8eda566778b2a316c29","serverId":"nope"}"#,
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
        let long = format!(r#"{{"itemId":"{A}","pad":"{}"}}"#, "x".repeat(MAX_PARAMS));
        assert_eq!(parse(&long), None);
    }

    #[test]
    fn an_item_alone_waits_for_a_free_home_then_opens() {
        let p = link(true, None, None);
        assert_eq!(step_in(&p, 0, false, false, Some(0)), Step::Wait);
        assert_eq!(step_in(&p, 0, true, false, Some(0)), Step::Open);
    }

    #[test]
    fn drops_when_the_wait_is_over_even_off_home() {
        let p = link(true, None, None);
        assert_eq!(step_in(&p, 999, false, false, None), Step::Wait);
        assert!(matches!(step_in(&p, 1001, false, false, None), Step::Drop(_)));
        assert!(matches!(step_in(&p, 1001, true, false, Some(0)), Step::Drop(_)));
    }

    #[test]
    fn another_server_is_dropped_but_an_unnamed_one_is_not() {
        assert!(matches!(step_in(&link(true, None, Some(A)), 0, true, false, Some(0)), Step::Drop(_)));
        assert_eq!(step_in(&link(true, None, Some(B)), 0, true, false, Some(0)), Step::Open);
        assert_eq!(step_in(&link(true, None, None), 0, true, false, Some(0)), Step::Open);
    }

    #[test]
    fn the_named_user_is_picked_from_the_picker_and_then_waited_for() {
        let mut p = link(true, Some(U2), None);
        assert_eq!(step_in(&p, 0, false, true, Some(0)), Step::Pick(1));
        p.picked = true;
        assert_eq!(step_in(&p, 0, false, false, Some(1)), Step::Wait, "the handoff is landing");
        assert_eq!(step_in(&p, 0, true, false, Some(0)), Step::Wait, "still the old user's Home");
        assert_eq!(step_in(&p, 0, true, false, Some(1)), Step::Open);
        let mut p = link(false, Some(U2), None);
        p.picked = true;
        assert_eq!(step_in(&p, 0, true, false, Some(1)), Step::Done);
    }

    #[test]
    fn from_home_another_user_asks_for_the_picker_once() {
        let mut p = link(true, Some(U2), None);
        assert_eq!(step_in(&p, 0, true, false, Some(0)), Step::Seat);
        p.seated = true;
        assert_eq!(step_in(&p, 0, true, false, Some(0)), Step::Wait, "the picker is still landing");
        assert_eq!(step_in(&p, 0, false, true, Some(0)), Step::Pick(1));
    }

    #[test]
    fn the_signed_in_user_needs_no_switch() {
        assert_eq!(step_in(&link(true, Some(U1), None), 0, true, false, Some(0)), Step::Open);
        assert_eq!(step_in(&link(false, Some(U1), None), 0, true, false, Some(0)), Step::Done);
        assert_eq!(step_in(&link(false, Some(U1), None), 0, false, true, Some(0)), Step::Pick(0), "skip the picker");
    }

    #[test]
    fn a_user_that_is_not_kept_or_on_another_server_is_dropped() {
        assert!(matches!(step_in(&link(true, Some(A), None), 0, true, false, Some(0)), Step::Drop(_)));
        assert!(matches!(step_in(&link(true, Some(U1), Some(A)), 0, true, false, Some(0)), Step::Drop(_)));
        assert_eq!(step_in(&link(true, Some(U1), Some(B)), 0, true, false, Some(0)), Step::Open);
    }
}
