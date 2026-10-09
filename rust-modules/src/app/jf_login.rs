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
        // A server that answers only over plain http, in a build that will not send a credential
        // there on its own, is admitted at once when the person already allowed it (the probe has
        // just proved which server answers), and otherwise asked about.
        JfAuthCmd::Probe { candidates, reply } => answer("jf probe", reply, move || {
            let scope = crate::catalog::grant::scope();
            match crate::jf::auth::probe_first(&candidates, &client_id) {
                Ok((origin, info)) if crate::jf::plaintext::needs_consent(&origin) => {
                    let server_id = crate::jf::ids::normalize(&info.id);
                    match crate::jf::plaintext::admit_proved(scope, &origin, &server_id) {
                        Ok(()) => JfAuthReply::Probed(Ok((origin, info))),
                        Err(_) => {
                            let internet = crate::jf::plaintext::on_internet(&origin);
                            log(&format!("jf: {} answers only without encryption — asking", origin.log_form()));
                            JfAuthReply::Consent { origin, info, internet }
                        }
                    }
                }
                other => JfAuthReply::Probed(other),
            }
        }),
        JfAuthCmd::AllowPlaintext { origin, server_id, server_name, reply } => answer("jf allow plaintext", reply, move || {
            crate::jf::store::set_plaintext(&server_id, &server_name, &origin, true);
            let roster = crate::jf::store::roster();
            let _ = crate::jf::store::persist(&roster);
            log(&format!("jf: unencrypted connections allowed for the server at {}", origin.log_form()));
            JfAuthReply::Allowed(crate::jf::plaintext::admit(&origin, &server_id))
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
        JfAuthCmd::CheckKept { index, reply } => {
            let Some(user) = crate::jf::store::roster().users.get(usize::from(index)).cloned() else {
                let _ = reply.send(JfAuthReply::Checked(index, Err(crate::jf::auth::AuthError::Malformed)));
                return;
            };
            let Some(origin) = user.origin() else {
                let _ = reply.send(JfAuthReply::Checked(index, Err(crate::jf::auth::AuthError::Malformed)));
                return;
            };
            answer("jf check user", reply, move || {
                JfAuthReply::Checked(index, crate::jf::auth::check_token(&origin, &client_id, &user.token, &user.device_user))
            })
        }
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
    // their own DeviceId, exactly as after a sign-in. Revoking ends every unencrypted-connection
    // grant with the identity; the server was proved on this network already, so it is minted
    // again before the install asks for it.
    crate::catalog::revoke_all();
    crate::jf::plaintext::remint();
    crate::jf::seat::register_with(&origin, user.seat());
    log(&format!("jf: switching user at {} — installing the server", origin.log_form()));
    bridge.hand_off_jf(ready_creds(&origin, user.token));
}

/// **The kept user at `index` was refused by the server** (their token was revoked, or expired):
/// forget them here, and ask for their password again — the add-a-user screen, open at
/// *Sign in as name* with a line saying why. Whoever is signed in underneath stays.
pub(super) fn reauth_user(pages: &mut crate::ui::dispatch::Dispatcher<super::bridge::AppHost>,
    bridge: &mut super::bridge::Bridge, index: usize) {
    let Some(user) = crate::jf::store::roster().users.get(index).cloned() else { return };
    crate::jf::store::forget_user(&user.server, &user.user_id);
    let roster = crate::jf::store::roster();
    let _ = nj_base::storage_worker::submit_retained(move || crate::jf::store::persist(&roster));
    log("jf: who's watching — the server refused that user's sign-in; asking for it again");
    let name = if user.user_name.is_empty() { user.user_id.clone() } else { user.user_name.clone() };
    super::bridge::open_login_as(pages, bridge, crate::screens::jf_login::Opening::SignInAgain {
        server: user.server, server_name: user.server_name, id: user.user_id, name });
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
    crate::catalog::revoke_all();
    // Revoking ends every unencrypted-connection grant with the identity. The server was proved on
    // this network already: mint again, so the sign-out below reaches it, and whoever is still
    // kept picks themselves next over the same connection.
    crate::jf::plaintext::remint();
    revoke(live, false);
    left
}

/// Sign-out for Delete all local data: every kept user and the server go, and every stored copy
/// with them. Each kept user's token is revoked at the server. Nothing is written back — the
/// session file is about to be deleted, and a write queued behind that delete would bring it back.
pub(super) fn sign_out_and_forget_server() {
    let users = crate::jf::store::roster().users;
    // Each sign-out below may need the answer this erases (a server reached without encryption):
    // read it first, and let each worker prove its server once more for that last request.
    let allowed: Vec<bool> = users
        .iter()
        .map(|u| u.origin().is_some_and(|o| {
            crate::jf::store::plaintext_choice(&crate::jf::ids::normalize(&u.server_id), &o) == Some(true)
        }))
        .collect();
    crate::jf::store::forget_everything();
    let _ = nj_base::storage_worker::submit_retained(crate::jf::store::erase);
    crate::catalog::revoke_all();
    for (user, allowed) in users.into_iter().zip(allowed) {
        revoke(user, allowed);
    }
}

/// Tell the server to revoke `stored`'s token, on a worker, and forget its seat here.
/// `last_allowed`: the person's (now erased) answer for a server reached without encryption —
/// Delete all local data's last request to it; the grant it needs is withdrawn again after.
fn revoke(stored: Stored, last_allowed: bool) {
    let Some(origin) = stored.origin() else { return };
    crate::jf::seat::forget(&origin);
    let client_id = crate::catalog::session::peek().client_id.clone();
    nj_base::task::spawn_small("jf sign-out", move || {
        let server_id = crate::jf::ids::normalize(&stored.server_id);
        if last_allowed {
            let _ = crate::jf::plaintext::admit_for_sign_out(&origin, &server_id, true);
        }
        let revoked = crate::jf::auth::sign_out_detached(&origin, &client_id, &stored.token, &stored.device_user);
        if last_allowed {
            crate::jf::plaintext::withdraw(&server_id);
        }
        log(if revoked { "jf: signed out — the server revoked this device's token" }
            else { "jf: signed out — the server could not be told (the token stays valid there)" });
    });
}

/// The re-admission of a server reached without encryption: one attempt in flight at most, the
/// next one not before `due` (frame ms).
struct Readmit {
    in_flight: bool,
    due: u32,
    attempt: u32,
}

static READMIT: std::sync::Mutex<Readmit> = std::sync::Mutex::new(Readmit { in_flight: false, due: 0, attempt: 0 });

/// **Keep the active user's unencrypted connection admitted**, once per frame. Its grant dies with
/// the network (the app returning to the foreground) and is never minted when the server could not
/// be proved (it was off at boot), and Settings can turn it back on; in each case the server is
/// proved again here on a worker — never by asking the person again — and the token the registry
/// blanked is put back. Backs off from 5 s to a minute between attempts; costs one lock when there
/// is nothing to do.
pub(super) fn step_plaintext(now: u32) {
    let Some(user) = crate::jf::store::current() else { return };
    let Some(origin) = user.origin() else { return };
    if !crate::jf::plaintext::needs_consent(&origin) || crate::jf::plaintext::granted(&origin) {
        return;
    }
    let server_id = crate::jf::ids::normalize(&user.server_id);
    if crate::jf::store::plaintext_choice(&server_id, &origin) != Some(true) {
        return;
    }
    {
        let mut r = READMIT.lock().unwrap_or_else(|e| e.into_inner());
        if r.in_flight || now.wrapping_sub(r.due) > u32::MAX / 2 {
            return;
        }
        r.in_flight = true;
    }
    nj_base::task::spawn_small("jf readmit", move || {
        let admitted = crate::jf::plaintext::admit(&origin, &server_id).is_ok();
        if admitted {
            // The registry blanked this server's token when its grant ended: put it back.
            crate::catalog::install(&origin, &user.token, None, crate::catalog::ConnectionFacts::default());
            log(&format!("jf: unencrypted connection to {} proved again — reconnected", origin.log_form()));
        }
        let mut r = READMIT.lock().unwrap_or_else(|e| e.into_inner());
        r.in_flight = false;
        r.attempt = if admitted { 0 } else { (r.attempt + 1).min(4) };
        let wait_ms = 5_000u32 << r.attempt.min(3);
        r.due = now.wrapping_add(wait_ms.min(60_000));
        drop(r);
        nj_machine::idle::invalidate();
    });
}
