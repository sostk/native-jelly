//! Live executor for the Jellyfin sign-in screen (`screens::jf_login`), and the two account moves
//! that are Jellyfin's own: adopting a sign-in and signing out of one.
//!
//! The Session owner speaks plex.tv only, so a Jellyfin sign-in never passes through it. Adopting
//! one registers the seat, keeps the token (`jf::store`) and queues the same landing a Session
//! handoff takes (`bridge::follow_auth_landing`), which installs the server and enters Home.
use crate::jf::store::Stored;
use crate::screens::registry::{JfAuthCmd, JfAuthReply};
use nj_base::eventlog::log;
use std::sync::mpsc::Sender;

fn answer(what: &str, reply: Sender<JfAuthReply>, work: impl FnOnce() -> JfAuthReply + Send + 'static) {
    // A refused spawn drops the sender; the screen reads the disconnect as a failed step.
    nj_base::task::spawn_small(what, move || {
        let _ = reply.send(work());
        nj_machine::idle::invalidate();
    });
}

pub(super) fn execute(bridge: &mut super::bridge::Bridge, command: JfAuthCmd) {
    let client_id = crate::catalog::session::peek().client_id.clone();
    match command {
        JfAuthCmd::Probe { candidates, reply } => answer("jf probe", reply, move || {
            JfAuthReply::Probed(crate::jf::auth::probe_first(&candidates, &client_id))
        }),
        JfAuthCmd::Password { origin, username, password, reply } => answer("jf sign-in", reply, move || {
            JfAuthReply::SignedIn(crate::jf::auth::sign_in_with_password(&origin, &client_id, &username, &password))
        }),
        JfAuthCmd::QuickConnectStart { origin, reply } => answer("jf quick connect", reply, move || {
            JfAuthReply::QuickConnect(crate::jf::auth::quick_connect_start(&origin, &client_id))
        }),
        JfAuthCmd::QuickConnectPoll { origin, qc, reply } => answer("jf quick connect", reply, move || {
            JfAuthReply::Polled(crate::jf::auth::quick_connect_poll(&origin, &client_id, &qc))
        }),
        JfAuthCmd::Adopt { origin, signed_in } => adopt(bridge, origin, signed_in),
        JfAuthCmd::PublicUsers { origin, reply } => answer("jf public users", reply, move || {
            JfAuthReply::People(crate::jf::auth::public_users(&origin, &client_id))
        }),
    }
}

fn adopt(bridge: &mut super::bridge::Bridge, origin: crate::catalog::Origin, signed_in: crate::jf::auth::SignedIn) {
    crate::jf::seat::register_with(&origin, signed_in.seat());
    let stored = Stored::new(&origin, &signed_in);
    crate::jf::store::set_live(Some(stored));
    // Persist the whole roster: this sign-in is now the active user, and anyone signed in on this
    // TV before stays remembered beside it.
    let roster = crate::jf::store::roster();
    let _ = nj_base::storage_worker::submit_retained(move || crate::jf::store::persist(&roster));
    log(&format!("jf: signed in at {} — installing the server", origin.log_form()));
    bridge.hand_off_jf(ready_creds(&origin, signed_in.token));
}

/// The install a Jellyfin server takes: itself as the primary, no extras, its tier read off the
/// address as the dev boot does (there is no plex.tv connection list to read one from).
pub(super) fn ready_creds(origin: &crate::catalog::Origin, token: String) -> crate::auth::ReadyCreds {
    crate::auth::ReadyCreds {
        install: crate::auth::owner::ReadyInstall::PrimaryAndExtras(Vec::new()),
        origin: origin.clone(),
        address: origin.host().to_owned(),
        token,
        tier: Some(crate::catalog::probe::configured_tier(origin.host())),
        pin: None,
    }
}

/// **Choose the kept user at `index`** on the who's-watching screen. The user already running
/// enters straight away; another one becomes the active user and takes the same handoff a
/// sign-in does (`bridge::follow_auth_landing`), which reinstalls the server under their token
/// and drops every page of the previous user.
pub(super) fn pick_user(pages: &mut crate::ui::dispatch::Dispatcher<super::bridge::AppHost>,
    bridge: &mut super::bridge::Bridge, index: usize) {
    let roster = crate::jf::store::roster();
    let Some(user) = roster.users.get(index).cloned() else {
        log("jf: who's watching — that user is no longer kept");
        return;
    };
    let Some(origin) = user.origin() else { return };
    if roster.current().is_some_and(|live| live.same_user(&user)) {
        log("jf: who's watching — the signed-in user carries on");
        enter_signed_in(pages, bridge);
        return;
    }
    crate::jf::store::activate(&user.server, &user.user_id);
    let roster = crate::jf::store::roster();
    let _ = nj_base::storage_worker::submit_retained(move || crate::jf::store::persist(&roster));
    // The previous user's clients go; the server is installed again under this user's token and
    // their own DeviceId, exactly as after a sign-in.
    crate::catalog::revoke_all();
    crate::jf::seat::register_with(&origin, user.seat());
    log(&format!("jf: switching user at {} — installing the server", origin.log_form()));
    bridge.hand_off_jf(ready_creds(&origin, user.token));
}

/// The signed-in user leaves the who's-watching screen as themselves: on to their Home, or their
/// first-run Favourites when they have never answered it — the landing a sign-in takes, without
/// installing anything again.
fn enter_signed_in(pages: &mut crate::ui::dispatch::Dispatcher<super::bridge::AppHost>,
    bridge: &mut super::bridge::Bridge) {
    use crate::screens::registry::AppArg;
    super::input::maybe_ask_consent(pages);
    bridge.refresh_browse_directory();
    let to = if crate::stores::browse::onboard::asks(bridge.browse_directory()) { AppArg::Onboard } else { AppArg::Home };
    super::bridge::nav_root(pages, to);
}

/// What a Jellyfin sign-out left on this television.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SignedOut {
    /// No Jellyfin user was signed in: the caller signs out of plex.tv instead.
    NotJellyfin,
    /// Other users are still kept: the who's-watching screen offers them.
    OthersRemain,
    /// Nobody is kept: the sign-in screen, with the server under *Recent*.
    Nobody,
}

/// **Sign the active Jellyfin user out**: forget their sign-in here at once, tell the server on a
/// worker, and drop every registered client. Everyone else kept on this television stays, and so
/// do the server's address and name, for the sign-in screen's *Recent* row.
pub(super) fn sign_out() -> SignedOut {
    let Some(live) = crate::jf::store::current() else { return SignedOut::NotJellyfin };
    crate::jf::store::forget_user(&live.server, &live.user_id);
    let roster = crate::jf::store::roster();
    let left = if roster.users.is_empty() { SignedOut::Nobody } else { SignedOut::OthersRemain };
    let _ = nj_base::storage_worker::submit_retained(move || crate::jf::store::persist(&roster));
    revoke(live);
    crate::catalog::revoke_all();
    left
}

/// Sign-out for Delete all local data: every kept user and the server go, and every stored copy
/// with them. Each kept user's token is revoked at the server. Nothing is written back — the
/// session file is about to be deleted, and a write queued behind that delete would bring it back.
pub(super) fn sign_out_and_forget_server() {
    let users = crate::jf::store::roster().users;
    crate::jf::store::forget_everything();
    let _ = nj_base::storage_worker::submit_retained(crate::jf::store::erase);
    for user in users {
        revoke(user);
    }
    crate::catalog::revoke_all();
}

/// Tell the server to revoke `stored`'s token, on a worker, and forget its seat here.
fn revoke(stored: Stored) {
    let Some(origin) = stored.origin() else { return };
    crate::jf::seat::forget(&origin);
    let client_id = crate::catalog::session::peek().client_id.clone();
    nj_base::task::spawn_small("jf sign-out", move || {
        let revoked = crate::jf::auth::sign_out_detached(&origin, &client_id, &stored.token, &stored.device_user);
        log(if revoked { "jf: signed out — the server revoked this device's token" }
            else { "jf: signed out — the server could not be told (the token stays valid there)" });
    });
}
