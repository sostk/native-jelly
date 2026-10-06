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
    }
}

fn adopt(bridge: &mut super::bridge::Bridge, origin: crate::catalog::Origin, signed_in: crate::jf::auth::SignedIn) {
    crate::jf::seat::register_with(&origin, signed_in.seat());
    let stored = Stored::new(&origin, &signed_in);
    crate::jf::store::set_live(Some(stored.clone()));
    let _ = nj_base::storage_worker::submit_retained(move || crate::jf::store::persist(&stored));
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

/// **Sign out of the live Jellyfin sign-in**, if there is one: forget it here at once, tell the
/// server on a worker, and drop every registered client. `false` when no Jellyfin sign-in is live
/// (the caller signs out of plex.tv instead).
pub(super) fn sign_out() -> bool {
    let Some(stored) = crate::jf::store::current() else { return false };
    crate::jf::store::set_live(None);
    let _ = nj_base::storage_worker::submit_retained(crate::jf::store::erase);
    if let Some(origin) = stored.origin() {
        crate::jf::seat::forget(&origin);
        let client_id = crate::catalog::session::peek().client_id.clone();
        nj_base::task::spawn_small("jf sign-out", move || {
            let revoked = crate::jf::auth::sign_out_detached(&origin, &client_id, &stored.token, &stored.device_user);
            log(if revoked { "jf: signed out — the server revoked this device's token" }
                else { "jf: signed out — the server could not be told (the token stays valid there)" });
        });
    }
    crate::catalog::revoke_all();
    true
}
