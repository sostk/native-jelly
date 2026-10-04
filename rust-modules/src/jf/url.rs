//! The Jellyfin credential choke points: the `Authorization: MediaBrowser …` header and the one
//! query form, `ApiKey=`.
//!
//! Jellyfin 12.0 removed every other spelling — the `api_key` query parameter and the
//! `X-Emby-Token` / `X-MediaBrowser-Token` / `X-Emby-Authorization` headers answer 401, and the
//! `/emby` route prefix 404s (measured against 12.0.0, `docs/jf-spikes.md` S1). Both forms here are
//! accepted by 10.10 and 12.x alike, so one code path serves the whole supported range.
//!
//! `ApiKey=` is appended LAST, the shape `plex::Client::with_token` gives `X-Plex-Token`, so the
//! log scrubber and the poster store's re-keying find it where they look.

/// The query parameter name, exactly as the server spells it.
pub const API_KEY: &str = "ApiKey";

/// Does `path` carry an `ApiKey=` (or the legacy `api_key=`) query parameter? Matched as a whole
/// parameter name after `?` or `&`, case-insensitively, so `/Items/apikeyring` cannot trip it.
pub fn has_api_key(path: &str) -> bool {
    let Some((_, query)) = path.split_once('?') else { return false };
    query.split('&').any(|kv| {
        let name = kv.split('=').next().unwrap_or("");
        kv.contains('=') && (name.eq_ignore_ascii_case("apikey") || name.eq_ignore_ascii_case("api_key"))
    })
}

/// Append `ApiKey={token}` with the right separator. An empty token appends nothing: an anonymous
/// request (`/System/Info/Public`, artwork) must not carry an empty credential parameter.
pub fn with_api_key(path: &str, token: &str) -> String {
    if token.is_empty() {
        return path.to_string();
    }
    let sep = if path.contains('?') { '&' } else { '?' };
    format!("{path}{sep}{API_KEY}={}", crate::plex::urlenc_str(token))
}

/// The four device facts every Jellyfin request names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceIdentity {
    pub client: String,
    pub device: String,
    pub device_id: String,
    pub version: String,
}

/// `DeviceId` for one (installation, user) pair. **Jellyfin keeps one access token per DeviceId**
/// and a sign-in on a DeviceId revokes the previous token issued to it, so two users of one set
/// sharing the install's id would sign each other out. Deriving it per user keeps every seat its own
/// device while staying stable across launches.
pub fn device_id(install_id: &str, username: &str) -> String {
    let digest = plx_base::sha256::sha256(format!("{install_id}\n{}", username.to_lowercase()).as_bytes());
    digest[..16].iter().map(|b| format!("{b:02x}")).collect()
}

/// `MediaBrowser Client="…", Device="…", DeviceId="…", Version="…"[, Token="…"]` — every value
/// percent-encoded inside its quotes (the server URL-decodes them), so a device name with a comma
/// or quote cannot break the header apart.
pub fn authorization(id: &DeviceIdentity, token: &str) -> String {
    let enc = crate::plex::urlenc_str;
    let mut v = format!(
        "Authorization: MediaBrowser Client=\"{}\", Device=\"{}\", DeviceId=\"{}\", Version=\"{}\"",
        enc(&id.client),
        enc(&id.device),
        enc(&id.device_id),
        enc(&id.version)
    );
    if !token.is_empty() {
        v.push_str(&format!(", Token=\"{}\"", enc(token)));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ident() -> DeviceIdentity {
        DeviceIdentity {
            client: "PlxJF".into(),
            device: "LG \"OLED\", living room".into(),
            device_id: "abc".into(),
            version: "0.7.0".into(),
        }
    }

    #[test]
    fn the_key_is_appended_last_with_the_right_separator() {
        assert_eq!(with_api_key("/Users/Me", "t0k"), "/Users/Me?ApiKey=t0k");
        assert_eq!(with_api_key("/Items?limit=1", "t0k"), "/Items?limit=1&ApiKey=t0k");
        assert_eq!(with_api_key("/System/Info/Public", ""), "/System/Info/Public");
    }

    #[test]
    fn only_a_real_parameter_counts_as_a_credential() {
        assert!(has_api_key("/Videos/x/stream?static=true&ApiKey=abc"));
        assert!(has_api_key("/x?apikey=abc"));
        assert!(has_api_key("/x?api_key=abc"));
        assert!(!has_api_key("/Items/apikey"));
        assert!(!has_api_key("/x?NotApiKey=1"));
        assert!(!has_api_key("/x?ApiKey"));
    }

    #[test]
    fn the_header_quotes_and_encodes_every_value() {
        let h = authorization(&ident(), "");
        assert!(h.starts_with("Authorization: MediaBrowser Client=\"PlxJF\""), "{h}");
        assert!(h.contains("Device=\"LG%20%22OLED%22%2C%20living%20room\""), "{h}");
        assert!(!h.contains("Token="));
        assert!(authorization(&ident(), "s3cret").ends_with(", Token=\"s3cret\""));
    }

    #[test]
    fn a_device_id_is_stable_per_user_and_distinct_between_users() {
        let a = device_id("install-1", "Alice");
        assert_eq!(a, device_id("install-1", "alice"), "usernames are case-insensitive");
        assert_ne!(a, device_id("install-1", "bob"));
        assert_ne!(a, device_id("install-2", "alice"));
        assert_eq!(a.len(), 32);
    }

    #[test]
    fn neither_credential_form_survives_into_the_event_log() {
        let line = format!(
            "GET /Videos/x/stream?static=true&ApiKey=SECRETTOKEN0 {}",
            authorization(&ident(), "HEADERTOKEN1")
        );
        let out = plx_base::eventlog::scrub::scrub_local_with(&line, &[]);
        assert!(!out.contains("SECRETTOKEN0"), "{out}");
        assert!(!out.contains("HEADERTOKEN1"), "{out}");
    }
}
