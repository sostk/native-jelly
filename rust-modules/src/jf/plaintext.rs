//! **Connecting to a Jellyfin server without encryption** — only when the person said so, only to
//! the server they said it about, and only at the address they were asked about.
//!
//! A store build never lets a credential travel over plain http on its own
//! ([`crate::catalog::origin::CredentialPolicy::HttpsOnly`]); every request is checked at the
//! transport (`crate::http::credential_transport_allowed`). A home Jellyfin server usually speaks
//! only http, so without this module its sign-in could never be sent. What this module adds is the
//! evidence for a grant (`crate::catalog::grant::mint_jellyfin`):
//!
//! * **consent** — the person's answer for this server's own id at this exact origin, kept in the
//!   Jellyfin record (`super::store::PlaintextAllow`) and shown in Settings;
//! * **identity** — the anonymous `/System/Info/Public` at that origin, which carries no
//!   credential, answering with that same server id, under the generations the grant is bound to.
//!
//! The grant is memory only and dies with the identity (a sign-out, a switch) or the network (the
//! app returning to the foreground). [`admit`] re-proves identity over the network; [`remint`]
//! re-mints without asking the server again when it was proved on this same network generation
//! (a user switch changes who is signed in, not which machine answers at the address).
//! A developer build allows plaintext anyway and needs none of this ([`needs_consent`]).
use super::auth::AuthError;
use crate::catalog::origin::CredentialPolicy;
use crate::catalog::Origin;
use std::sync::Mutex;

/// The last proof that `server_id` answered at `origin`, and the generations it was made under.
struct Proved {
    server_id: String,
    origin: Origin,
    scope: crate::catalog::grant::GrantScope,
}

static PROVED: Mutex<Vec<Proved>> = Mutex::new(Vec::new());

/// How long the identity probe may take — it sits in front of a boot, so a server that is off must
/// not hold the app for a full request timeout.
const PROBE_TIMEOUT_S: i32 = 4;

/// Would a credential for `origin` need the person's consent in this build?
pub fn needs_consent(origin: &Origin) -> bool {
    // The build's policy alone, asked of the one authority (`catalog::grant`): a live grant does
    // not make the answer "no consent needed" — it is what the consent earned.
    !crate::catalog::grant::remembered_allowed(CredentialPolicy::build(), origin)
}

/// Is `origin` on the internet rather than a home network — for the consent step's wording, never
/// for a security decision (the grant is the same either way; the person decides). A name counts
/// as home only when everything it resolves to right now is a home-network address; one that does
/// not resolve is spoken of as the internet, the stronger warning.
pub fn on_internet(origin: &Origin) -> bool {
    use crate::catalog::probe::AddressScope;
    let home = |scope: AddressScope| scope.is_private_network() || scope == AddressScope::Loopback;
    match AddressScope::of(origin.host()).0 {
        AddressScope::Name => {
            use std::net::ToSocketAddrs;
            match (origin.host(), origin.port() as u16).to_socket_addrs() {
                Ok(addrs) => {
                    let scopes: Vec<_> = addrs.map(|a| AddressScope::of(&a.ip().to_string()).0).collect();
                    scopes.is_empty() || !scopes.into_iter().all(home)
                }
                Err(_) => true,
            }
        }
        scope => !home(scope),
    }
}

/// Ask `origin` who it is — anonymously, no credential — and check it is `server_id`.
pub fn verify(origin: &Origin, server_id: &str) -> Result<(), AuthError> {
    let reply = crate::http::request_probe(origin, "/System/Info/Public", crate::http::Method::Get,
        &[crate::http::ACCEPT_JSON], 64 * 1024, PROBE_TIMEOUT_S, None)
        .map_err(|_| AuthError::Unreachable)?;
    if !reply.ok() {
        return Err(AuthError::NotJellyfin);
    }
    let info: super::models::PublicSystemInfo = serde_json::from_slice(&reply.body).map_err(|_| AuthError::NotJellyfin)?;
    if super::ids::normalize(&info.id) != server_id {
        nj_base::eventlog::log("security: a different server answered at a consented plaintext address — not sending credentials");
        return Err(AuthError::NotJellyfin);
    }
    Ok(())
}

/// **Let credentials reach `server_id` at `origin`**, if the person allowed it: prove the server's
/// identity there now, then mint the grant under the generations captured BEFORE the proof (so a
/// network change during it leaves nothing behind). `Ok` straight away where no consent is needed.
pub fn admit(origin: &Origin, server_id: &str) -> Result<(), AuthError> {
    if !needs_consent(origin) {
        return Ok(());
    }
    if super::store::plaintext_choice(server_id, origin) != Some(true) {
        return Err(AuthError::NeedsConsent);
    }
    let scope = crate::catalog::grant::scope();
    verify(origin, server_id)?;
    mint(scope, server_id, origin)
}

/// **One last request to a server the person had allowed** — the sign-out that revokes a token,
/// sent after Delete all local data has already erased the answer. The answer is passed in, read
/// before it was erased; the server is proved again first, as for any grant.
pub fn admit_for_sign_out(origin: &Origin, server_id: &str, allowed: bool) -> Result<(), AuthError> {
    if !needs_consent(origin) || granted(origin) {
        return Ok(());
    }
    if !allowed {
        return Err(AuthError::NeedsConsent);
    }
    let scope = crate::catalog::grant::scope();
    verify(origin, server_id)?;
    mint(scope, server_id, origin)
}

/// [`admit`] for an answer the caller has JUST proved — the sign-in's own probe of `origin`
/// returned `server_id` under `scope`, captured before it asked.
pub(crate) fn admit_proved(scope: crate::catalog::grant::GrantScope, origin: &Origin, server_id: &str) -> Result<(), AuthError> {
    if !needs_consent(origin) {
        return Ok(());
    }
    if super::store::plaintext_choice(server_id, origin) != Some(true) {
        return Err(AuthError::NeedsConsent);
    }
    mint(scope, server_id, origin)
}

fn mint(scope: crate::catalog::grant::GrantScope, server_id: &str, origin: &Origin) -> Result<(), AuthError> {
    crate::catalog::grant::mint_jellyfin(scope, server_id, origin).map_err(|_| AuthError::NeedsConsent)?;
    let mut proved = PROVED.lock().unwrap_or_else(|e| e.into_inner());
    proved.retain(|p| !(p.server_id == server_id && p.origin == *origin));
    proved.push(Proved { server_id: server_id.to_owned(), origin: origin.clone(), scope });
    Ok(())
}

/// After the identity moved (a sign-out, a user switch: `catalog::revoke_all`), mint again every
/// grant whose server was proved on THIS network generation and is still allowed. No network:
/// the machine at the address was proved already; only who is signed in changed.
pub fn remint() {
    let now = crate::catalog::grant::scope();
    let proved: Vec<(String, Origin)> = PROVED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|p| p.scope.same_network(now))
        .map(|p| (p.server_id.clone(), p.origin.clone()))
        .collect();
    for (server_id, origin) in proved {
        if super::store::plaintext_choice(&server_id, &origin) == Some(true) {
            let _ = mint(now, &server_id, &origin);
        }
    }
}

/// Withdraw `server_id`'s grant now — the person turned it off in Settings.
pub fn withdraw(server_id: &str) {
    PROVED.lock().unwrap_or_else(|e| e.into_inner()).retain(|p| p.server_id != server_id);
    crate::catalog::grant::revoke(&crate::catalog::grant::jellyfin_machine(server_id));
}

/// Is `origin` credentialed right now?
pub fn granted(origin: &Origin) -> bool {
    crate::catalog::grant::credential_allowed(origin)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The consent step's wording: a home-network literal or this television itself is "home";
    /// any other literal is the internet.
    #[test]
    fn literals_are_home_or_internet_by_their_address() {
        for home in ["http://192.168.1.20:8096", "http://10.0.0.5:8096", "http://172.20.1.1:8096", "http://127.0.0.1:8096", "http://[fd00::5]:8096"] {
            assert!(!on_internet(&Origin::parse(home).unwrap()), "{home}");
        }
        for internet in ["http://203.0.113.9:8096", "http://8.8.8.8:80"] {
            assert!(on_internet(&Origin::parse(internet).unwrap()), "{internet}");
        }
        assert!(on_internet(&Origin::parse("http://name.invalid:8096").unwrap()), "a name that resolves to nothing gets the stronger warning");
    }

    /// The re-mint after a sign-out or a switch needs the person's answer AND a proof on this same
    /// network generation; it never proves anything itself.
    #[test]
    fn remint_needs_the_answer_and_a_proof_on_this_network() {
        let _g = nj_base::testlock::serial();
        crate::catalog::grant::reset_for_test();
        super::super::store::forget_everything();
        PROVED.lock().unwrap().clear();
        let origin = Origin::parse("http://192.168.1.20:8096").unwrap();
        let machine = crate::catalog::grant::jellyfin_machine("abc");
        mint(crate::catalog::grant::scope(), "abc", &origin).unwrap();
        crate::catalog::grant::identity_changed();
        remint();
        assert!(crate::catalog::grant::granted_origin(&machine).is_none(), "no answer recorded: nothing minted");
        super::super::store::set_plaintext("abc", "Living Room", &origin, true);
        remint();
        assert_eq!(crate::catalog::grant::granted_origin(&machine), Some(origin.clone()), "allowed and proved on this network");
        crate::catalog::grant::network_changed();
        remint();
        assert!(crate::catalog::grant::granted_origin(&machine).is_none(), "a new network needs a new proof");
        withdraw("abc");
        super::super::store::forget_everything();
        crate::catalog::grant::reset_for_test();
        PROVED.lock().unwrap().clear();
    }
}
