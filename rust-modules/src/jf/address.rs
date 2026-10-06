//! **What a typed server address could mean**, as the origins to try in order.
//!
//! `Origin::parse` fills a missing port with PMS's 32400, which is never where Jellyfin listens, and
//! a person at a television types `192.168.1.20` or `jellyfin.example.com` far more often than a
//! whole URL. So a missing scheme or port becomes a short list instead of one guess: Jellyfin's own
//! ports (8096 plaintext, 8920 TLS) and the reverse-proxy ones (80, 443), the likely one first — a
//! LAN address is usually the bare server, a domain name usually sits behind https.
use crate::catalog::{Origin, Scheme};

/// The origins `input` may name, best guess first. Empty when it cannot be an address at all.
pub fn candidates(input: &str) -> Vec<Origin> {
    let s = input.trim();
    let (scheme, rest) = match s.split_once("://") {
        Some((scheme, rest)) => match scheme.to_ascii_lowercase().as_str() {
            "http" => (Some(false), rest),
            "https" => (Some(true), rest),
            _ => return Vec::new(),
        },
        None => (None, s),
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or("");
    let Some((host, port)) = split_host_port(authority) else { return Vec::new() };
    if host.is_empty() || host.chars().any(|c| c.is_whitespace()) {
        return Vec::new();
    }
    let plain = |p: i32| (false, p);
    let tls = |p: i32| (true, p);
    let tries: Vec<(bool, i32)> = match (scheme, port) {
        (Some(t), Some(p)) => vec![(t, p)],
        (Some(false), None) => vec![plain(8096), plain(80)],
        (Some(true), None) => vec![tls(443), tls(8920)],
        (None, Some(p)) if p == 443 || p == 8920 => vec![tls(p), plain(p)],
        (None, Some(p)) => vec![plain(p), tls(p)],
        (None, None) if looks_local(host) => vec![plain(8096), tls(8920), plain(80), tls(443)],
        (None, None) => vec![tls(443), plain(8096), tls(8920), plain(80)],
    };
    tries
        .into_iter()
        .map(|(tls, port)| {
            Origin::new(if tls { Scheme::Https } else { Scheme::Http }, host, port)
        })
        .collect()
}

/// `host[:port]`, a v6 literal bracketed. `None` for a written port that is not dialable.
fn split_host_port(authority: &str) -> Option<(&str, Option<i32>)> {
    let port = |p: &str| p.parse::<i32>().ok().filter(|p| (1..=65535).contains(p));
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        return match after.strip_prefix(':') {
            Some(p) => Some((host, Some(port(p)?))),
            None if after.is_empty() => Some((host, None)),
            None => None,
        };
    }
    match authority.matches(':').count() {
        0 => Some((authority, None)),
        1 => {
            let (host, p) = authority.split_once(':')?;
            Some((host, Some(port(p)?)))
        }
        // An unbracketed v6 literal carries no port.
        _ => Some((authority, None)),
    }
}

/// An address that is almost certainly on this network: an IP literal, `localhost`, a dotless
/// name, or an mDNS `.local` / `.lan` / `.home` name.
fn looks_local(host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    h.parse::<std::net::IpAddr>().is_ok()
        || !h.contains('.')
        || [".local", ".lan", ".home", ".internal"].iter().any(|s| h.ends_with(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bases(input: &str) -> Vec<String> {
        candidates(input).iter().map(Origin::base).collect()
    }

    #[test]
    fn a_whole_url_is_taken_at_its_word() {
        assert_eq!(bases("http://10.0.0.2:8096"), ["http://10.0.0.2:8096"]);
        assert_eq!(bases("  HTTPS://jf.example.com:8443/web/index.html "), ["https://jf.example.com:8443"]);
    }

    #[test]
    fn a_missing_port_tries_jellyfin_before_the_proxy_port() {
        assert_eq!(bases("http://10.0.0.2"), ["http://10.0.0.2:8096", "http://10.0.0.2:80"]);
        assert_eq!(bases("https://jf.example.com/"), ["https://jf.example.com:443", "https://jf.example.com:8920"]);
    }

    #[test]
    fn a_bare_lan_address_is_the_server_itself_first() {
        assert_eq!(
            bases("192.168.1.20"),
            ["http://192.168.1.20:8096", "https://192.168.1.20:8920", "http://192.168.1.20:80", "https://192.168.1.20:443"]
        );
        assert_eq!(bases("nas")[0], "http://nas:8096");
        assert_eq!(bases("media.local")[0], "http://media.local:8096");
    }

    #[test]
    fn a_bare_domain_is_https_first() {
        assert_eq!(bases("jellyfin.example.com")[0], "https://jellyfin.example.com:443");
    }

    #[test]
    fn a_bare_port_decides_the_scheme_order() {
        assert_eq!(bases("10.0.0.2:8096"), ["http://10.0.0.2:8096", "https://10.0.0.2:8096"]);
        assert_eq!(bases("10.0.0.2:8920"), ["https://10.0.0.2:8920", "http://10.0.0.2:8920"]);
    }

    #[test]
    fn v6_literals_keep_their_brackets_out_of_the_host() {
        assert_eq!(bases("http://[fd00::2]:8096"), ["http://[fd00::2]:8096"]);
        assert_eq!(bases("fd00::2")[0], "http://[fd00::2]:8096");
    }

    #[test]
    fn nonsense_is_no_address() {
        for s in ["", "   ", "http://", "ftp://x", "10.0.0.2:99999", "10.0.0.2:abc", "my server"] {
            assert!(candidates(s).is_empty(), "{s:?}");
        }
    }
}
