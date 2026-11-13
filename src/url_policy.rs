//! Policy for API base URLs: credentials must not travel in cleartext.
//!
//! `https://` is accepted for any host. `http://` is accepted only when the
//! host is a loopback address (`localhost`, `127.0.0.0/8`, `::1`), which is
//! what local development and test servers use. Everything else is refused.

use url::{Host, Url};

use crate::errors::{QuomeError, Result};

/// Parse `raw` and check it against the policy. `what` names the variable or
/// setting the value came from, for the error message. Returns the URL with
/// surrounding whitespace and trailing slashes removed.
pub fn validate_api_url(raw: &str, what: &str) -> Result<String> {
    let trimmed = raw.trim();
    let url = Url::parse(trimmed).map_err(|e| {
        QuomeError::Usage(format!(
            "{what} is not a valid URL ({e}); use an https:// URL, got {trimmed:?}"
        ))
    })?;
    match url.scheme() {
        "https" => {}
        "http" if is_loopback(&url) => {}
        "http" => {
            return Err(QuomeError::Usage(format!(
                "{what} uses plain http://, which would send credentials unencrypted; \
                 use an https:// URL (http:// is only allowed for localhost), got {trimmed:?}"
            )))
        }
        other => {
            return Err(QuomeError::Usage(format!(
                "{what} must be an https:// URL, got scheme {other:?} in {trimmed:?}"
            )))
        }
    }
    if url.host_str().is_none() {
        return Err(QuomeError::Usage(format!(
            "{what} has no host; use an https:// URL, got {trimmed:?}"
        )));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(QuomeError::Usage(format!(
            "{what} must not contain a username or password"
        )));
    }
    Ok(trimmed.trim_end_matches('/').to_string())
}

/// True when `raw` is a valid `http://` URL to a loopback host. Clients use
/// this to decide whether they may be built without `https_only`.
pub fn is_loopback_http(raw: &str) -> bool {
    Url::parse(raw.trim())
        .map(|u| u.scheme() == "http" && is_loopback(&u))
        .unwrap_or(false)
}

/// True when the URL's host is this computer (localhost or a loopback address).
pub fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(d)) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            d == "localhost" || d.ends_with(".localhost")
        }
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str) -> String {
        validate_api_url(s, "X").unwrap_or_else(|e| panic!("{s}: {e}"))
    }
    fn refused(s: &str) -> String {
        validate_api_url(s, "QUOME_API_URL")
            .expect_err(s)
            .to_string()
    }

    #[test]
    fn https_any_host() {
        assert_eq!(ok("https://api.example.com/"), "https://api.example.com");
        assert_eq!(ok(" https://203.0.113.5:8443 "), "https://203.0.113.5:8443");
    }

    #[test]
    fn http_remote_refused_naming_the_setting() {
        let e = refused("http://api.example.com");
        assert!(e.contains("QUOME_API_URL") && e.contains("https"), "{e}");
        refused("http://203.0.113.5");
        refused("http://localhost.example.com");
        refused("http://[2001:db8::1]");
    }

    #[test]
    fn http_loopback_accepted() {
        ok("http://localhost:8080");
        ok("http://127.0.0.1:9000");
        ok("http://127.5.6.7");
        ok("http://[::1]:3000");
    }

    #[test]
    fn scheme_is_case_insensitive() {
        refused("HTTP://api.example.com");
        ok("HTTP://127.0.0.1:1");
        ok("HTTPS://api.example.com");
    }

    #[test]
    fn userinfo_tricks_refused() {
        refused("http://127.0.0.1@evil.example");
        refused("http://localhost:80@evil.example/");
        refused("http://127.0.0.1:pw@evil.example");
        refused("https://user:pw@api.example.com");
    }

    #[test]
    fn other_schemes_and_garbage_refused() {
        refused("ftp://api.example.com");
        refused("api.example.com");
        refused("");
        refused("file:///etc/passwd");
    }

    #[test]
    fn loopback_http_detection() {
        assert!(is_loopback_http("http://127.0.0.1:1"));
        assert!(is_loopback_http("http://[::1]"));
        assert!(!is_loopback_http("https://127.0.0.1"));
        assert!(!is_loopback_http("http://127.0.0.1@evil.example"));
        assert!(!is_loopback_http("http://evil.example"));
    }
}
