//! Signing in to a Jellyfin server: the anonymous probe that classifies an address, password and
//! QuickConnect sign-in, and sign-out.
//!
//! Every call goes through an unregistered `plex::Client` for the origin, so it takes the same
//! transport, resolve pin and plaintext-credential authority as everything after it. A password
//! is sent only in the JSON body of `POST /Users/AuthenticateByName` — never in a URL — and
//! [`SignedIn::token`] is the one secret the caller has to keep.
use super::api::Jf;
use super::models::*;
use super::seat;
use crate::http::Method;
use crate::plex::Origin;
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    /// Nothing answered (or the credential authority refused a plaintext send).
    Unreachable,
    /// Something answered, but not as a Jellyfin server.
    NotJellyfin,
    /// The server has not finished its first-run wizard.
    NotSetUp,
    BadCredentials,
    QuickConnectDisabled,
    /// Any other HTTP status.
    Refused(i32),
    Malformed,
}

#[derive(Debug, Clone, Default)]
pub struct SignedIn {
    pub token: String,
    pub user_id: String,
    pub user_name: String,
    pub server_id: String,
    pub server_name: String,
    pub server_version: String,
    /// The DeviceId basis the token was minted under (see [`seat::Seat::device_user`]).
    pub device_user: String,
}

impl SignedIn {
    /// The seat this sign-in establishes for `origin` — register it before installing the client.
    pub fn seat(&self) -> seat::Seat {
        seat::Seat {
            user_id: self.user_id.clone(),
            server_id: super::ids::normalize(&self.server_id),
            server_name: self.server_name.clone(),
            server_version: self.server_version.clone(),
            user_name: self.user_name.clone(),
            device_user: self.device_user.clone(),
        }
    }
}

/// Is `origin` a Jellyfin server, and what does it call itself? Anonymous.
pub fn probe(origin: &Origin, client_id: &str) -> Result<PublicSystemInfo, AuthError> {
    let c = crate::plex::unregistered_client(origin.clone(), "", client_id);
    let r = c
        .jf_send("/System/Info/Public", Method::Get, &[crate::http::ACCEPT_JSON], None)
        .ok_or(AuthError::Unreachable)?;
    if !r.ok() {
        return Err(AuthError::NotJellyfin);
    }
    let info: PublicSystemInfo = serde_json::from_slice(&r.body).map_err(|_| AuthError::NotJellyfin)?;
    if info.id.is_empty() || !info.product_name.to_ascii_lowercase().contains("jellyfin") {
        return Err(AuthError::NotJellyfin);
    }
    if !info.startup_wizard_completed {
        return Err(AuthError::NotSetUp);
    }
    Ok(info)
}

fn finish(info: &PublicSystemInfo, a: AuthenticationResult, device_user: &str) -> Result<SignedIn, AuthError> {
    if a.access_token.is_empty() || a.user.id.is_empty() {
        return Err(AuthError::Malformed);
    }
    Ok(SignedIn {
        token: a.access_token,
        user_id: a.user.id,
        user_name: a.user.name,
        server_id: if a.server_id.is_empty() { info.id.clone() } else { a.server_id },
        server_name: info.server_name.clone(),
        server_version: info.version.clone(),
        device_user: device_user.to_string(),
    })
}

fn status_error(status: i32) -> AuthError {
    match status {
        401 | 403 => AuthError::BadCredentials,
        s => AuthError::Refused(s),
    }
}

/// `POST /Users/AuthenticateByName` — username and password in the body.
pub fn sign_in_with_password(origin: &Origin, client_id: &str, username: &str, password: &str) -> Result<SignedIn, AuthError> {
    let info = probe(origin, client_id)?;
    let device_user = username.trim().to_lowercase();
    let c = crate::plex::unregistered_client(origin.clone(), "", client_id);
    let j = Jf::for_sign_in(&c, &device_user);
    let body = serde_json::to_vec(&json!({ "Username": username.trim(), "Pw": password })).map_err(|_| AuthError::Malformed)?;
    let auth = j.auth_header();
    let r = c
        .jf_send("/Users/AuthenticateByName", Method::Post,
            &[crate::http::ACCEPT_JSON, auth.as_str(), "Content-Type: application/json"], Some(&body))
        .ok_or(AuthError::Unreachable)?;
    if !r.ok() {
        return Err(status_error(r.status));
    }
    let a: AuthenticationResult = serde_json::from_slice(&r.body).map_err(|_| AuthError::Malformed)?;
    finish(&info, a, &device_user)
}

/// A QuickConnect request in flight: show [`Self::code`], poll [`quick_connect_poll`].
#[derive(Debug, Clone)]
pub struct QuickConnect {
    pub code: String,
    secret: String,
    info: PublicSystemInfo,
}

/// `GET /QuickConnect/Enabled` then `POST /QuickConnect/Initiate`.
pub fn quick_connect_start(origin: &Origin, client_id: &str) -> Result<QuickConnect, AuthError> {
    let info = probe(origin, client_id)?;
    let c = crate::plex::unregistered_client(origin.clone(), "", client_id);
    let j = Jf::for_sign_in(&c, "");
    let auth = j.auth_header();
    let headers = [crate::http::ACCEPT_JSON, auth.as_str()];
    let enabled = c.jf_send("/QuickConnect/Enabled", Method::Get, &headers, None).ok_or(AuthError::Unreachable)?;
    if !enabled.ok() || enabled.body.trim_ascii() != b"true" {
        return Err(AuthError::QuickConnectDisabled);
    }
    let r = c.jf_send("/QuickConnect/Initiate", Method::Post, &headers, None).ok_or(AuthError::Unreachable)?;
    if !r.ok() {
        return Err(if r.status == 401 { AuthError::QuickConnectDisabled } else { AuthError::Refused(r.status) });
    }
    let q: QuickConnectResult = serde_json::from_slice(&r.body).map_err(|_| AuthError::Malformed)?;
    if q.secret.is_empty() || q.code.is_empty() {
        return Err(AuthError::Malformed);
    }
    Ok(QuickConnect { code: q.code, secret: q.secret, info })
}

/// One poll: `Ok(None)` while the code is not yet approved, `Ok(Some)` once it is.
pub fn quick_connect_poll(origin: &Origin, client_id: &str, qc: &QuickConnect) -> Result<Option<SignedIn>, AuthError> {
    let c = crate::plex::unregistered_client(origin.clone(), "", client_id);
    let j = Jf::for_sign_in(&c, "");
    let auth = j.auth_header();
    let path = super::api::Q::new("/QuickConnect/Connect").s("secret", &qc.secret).build();
    let r = c.jf_send(&path, Method::Get, &[crate::http::ACCEPT_JSON, auth.as_str()], None).ok_or(AuthError::Unreachable)?;
    if !r.ok() {
        return Err(status_error(r.status));
    }
    let state: QuickConnectResult = serde_json::from_slice(&r.body).map_err(|_| AuthError::Malformed)?;
    if !state.authenticated {
        return Ok(None);
    }
    let body = serde_json::to_vec(&json!({ "Secret": qc.secret })).map_err(|_| AuthError::Malformed)?;
    let r = c
        .jf_send("/Users/AuthenticateWithQuickConnect", Method::Post,
            &[crate::http::ACCEPT_JSON, auth.as_str(), "Content-Type: application/json"], Some(&body))
        .ok_or(AuthError::Unreachable)?;
    if !r.ok() {
        return Err(status_error(r.status));
    }
    let a: AuthenticationResult = serde_json::from_slice(&r.body).map_err(|_| AuthError::Malformed)?;
    finish(&qc.info, a, "").map(Some)
}

/// `POST /Sessions/Logout` — revoke this device's token on the server. Best effort.
pub fn sign_out(c: &crate::plex::Client) -> bool {
    match c.jf() {
        Some(j) => j.status("/Sessions/Logout", Method::Post, None).is_some_and(|s| (200..300).contains(&s)),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sign_in_becomes_the_seat_its_client_reads() {
        let s = SignedIn {
            token: "t".into(),
            user_id: "u".into(),
            user_name: "Cursor".into(),
            server_id: "AB-CD".into(),
            server_name: "Home".into(),
            server_version: "12.0.0".into(),
            device_user: "cursor".into(),
        };
        let seat = s.seat();
        assert_eq!((seat.server_id.as_str(), seat.device_user.as_str()), ("abcd", "cursor"));
    }

    #[test]
    fn statuses_map_to_what_the_login_screen_says() {
        assert_eq!(status_error(401), AuthError::BadCredentials);
        assert_eq!(status_error(500), AuthError::Refused(500));
    }
}
