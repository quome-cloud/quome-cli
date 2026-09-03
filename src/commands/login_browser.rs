//! `quome login --browser`: the browser approves, the key comes back over
//! loopback — no paste anywhere.
//!
//! 1. Mint a random `state` and a PKCE verifier; bind `127.0.0.1:0`.
//! 2. Open `{dashboard}/cli/authorize?kind=api_key&state&port&code_challenge&key_name`.
//!    The signed-in user picks an org (needs the Settings → admin capability
//!    there) and clicks **Create key**; the control plane redirects the tab to
//!    `http://127.0.0.1:{port}/callback?code&state`.
//! 3. Exchange the one-time code + verifier at `POST /api/v1/auth/cli/key`.
//!    The key is minted at that moment and returned exactly once.
//!
//! Same handoff `quome host login` uses, with `kind=api_key`.

use std::time::Duration;

use base64::Engine;
use rand::RngCore;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::api::models::{ApiKeySelf, CliApiKeyResponse, CliKeyExchangeRequest};
use crate::client::QuomeClient;
use crate::errors::{QuomeError, Result};

/// How long we wait for the browser round-trip before giving up. The
/// control plane's one-time code lives 120 s; the user also has to sign in
/// and pick an org, so allow a few minutes.
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);
const KEY_NAME_MAX: usize = 120;

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn random_token() -> String {
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    b64url(&buf)
}

/// RFC 7636 S256: base64url(sha256(verifier)) without padding.
pub fn s256(verifier: &str) -> String {
    b64url(&Sha256::digest(verifier.as_bytes()))
}

/// The name the key gets on the dashboard's API Keys page: `quome-cli@<host>`,
/// restricted to what the control plane accepts (`[A-Za-z0-9 ._@-]`, ≤120).
pub fn key_name_for(hostname: &str) -> String {
    let host: String = hostname
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .collect();
    let host = if host.is_empty() {
        "this-computer".to_string()
    } else {
        host
    };
    let name = format!("quome-cli@{host}");
    name.chars().take(KEY_NAME_MAX).collect()
}

pub fn authorize_url(
    dashboard: &str,
    state: &str,
    port: u16,
    challenge: &str,
    key_name: &str,
) -> String {
    let q = |s: &str| {
        s.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect::<String>()
    };
    format!(
        "{}/cli/authorize?kind=api_key&state={}&port={}&code_challenge={}&key_name={}",
        dashboard.trim_end_matches('/'),
        q(state),
        port,
        q(challenge),
        q(key_name)
    )
}

/// `code` and `state` from the first request line of the loopback callback
/// (`GET /callback?code=…&state=… HTTP/1.1`). Anything else is `None`.
pub fn parse_callback(request_line: &str) -> Option<(String, String)> {
    let path = request_line.split_whitespace().nth(1)?;
    let query = path.strip_prefix("/callback?")?;
    let mut code = None;
    let mut state = None;
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=')?;
        match k {
            "code" => code = Some(v.to_string()),
            "state" => state = Some(v.to_string()),
            _ => {}
        }
    }
    Some((code?, state?))
}

const CALLBACK_HTML: &str = "<!doctype html><meta charset=utf-8><title>Quome CLI</title>\
<body style=\"font-family:system-ui;margin:3rem\"><h2>Key sent to your terminal</h2>\
<p>You can close this tab and go back to <code>quome</code>.</p>";

async fn await_callback(listener: TcpListener, expected_state: &str) -> Result<String> {
    let (mut sock, _) = tokio::time::timeout(CALLBACK_TIMEOUT, listener.accept())
        .await
        .map_err(|_| {
            QuomeError::Usage(
                "Timed out waiting for the browser. Run `quome login --browser` again, \
                 or paste a key from Settings → API Keys with `quome login`."
                    .into(),
            )
        })??;
    let mut buf = vec![0u8; 4096];
    let n = sock.read(&mut buf).await?;
    let head = String::from_utf8_lossy(&buf[..n]);
    let first = head.lines().next().unwrap_or("");
    let parsed = parse_callback(first);
    let (status, body) = match &parsed {
        Some((_, state)) if state == expected_state => ("200 OK", CALLBACK_HTML),
        _ => ("400 Bad Request", "<p>Unexpected callback.</p>"),
    };
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = sock.write_all(resp.as_bytes()).await;
    let _ = sock.shutdown().await;
    match parsed {
        Some((code, state)) if state == expected_state => Ok(code),
        Some(_) => Err(QuomeError::Usage(
            "The browser came back with a different state than we sent — refusing it. \
             Run `quome login --browser` again."
                .into(),
        )),
        None => Err(QuomeError::Usage(
            "The browser callback was malformed.".into(),
        )),
    }
}

/// Run the whole handoff. Returns the raw key plus what it resolves to.
pub async fn run(dashboard_url: &str) -> Result<(String, ApiKeySelf)> {
    let state = random_token();
    let verifier = random_token();
    let challenge = s256(&verifier);
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let hostname = gethostname::gethostname().to_string_lossy().to_string();
    let key_name = key_name_for(&hostname);
    let url = authorize_url(dashboard_url, &state, port, &challenge, &key_name);

    println!("Opening your browser to approve a new API key ({key_name})…");
    if open::that(&url).is_err() {
        println!("Couldn't open a browser. Open this URL yourself:");
    } else {
        println!("If nothing opened, use this URL:");
    }
    println!("  {url}\n");

    let code = await_callback(listener, &state).await?;

    let client = QuomeClient::new(None, None)?;
    let resp: CliApiKeyResponse = client
        .exchange_cli_key(&CliKeyExchangeRequest {
            code,
            state,
            code_verifier: verifier,
        })
        .await
        .map_err(|e| match e {
            QuomeError::NotFound(_) => QuomeError::Usage(
                "Browser login isn't enabled on this control plane. \
                 Paste a key from Settings → API Keys with `quome login` instead."
                    .into(),
            ),
            other => other,
        })?;
    let identity = ApiKeySelf {
        org_id: resp.org_id,
        service_account_id: resp.service_account_id,
        scopes: resp.scopes,
        org_name: Some(resp.org_name),
        org_slug: Some(resp.org_slug),
    };
    Ok((resp.api_key, identity))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s256_matches_the_rfc_7636_vector() {
        assert_eq!(
            s256("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn random_tokens_are_long_enough_for_the_control_plane() {
        // state ≥ 32 chars, verifier ≥ 43 chars, both [A-Za-z0-9_-].
        let t = random_token();
        assert!(t.len() >= 43, "{t}");
        assert!(t
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert_ne!(random_token(), random_token());
    }

    #[test]
    fn key_name_is_sanitised_and_bounded() {
        assert_eq!(
            key_name_for("Jims-MacBook.local"),
            "quome-cli@Jims-MacBook.local"
        );
        assert_eq!(key_name_for("weird host!$"), "quome-cli@weirdhost");
        assert_eq!(key_name_for(""), "quome-cli@this-computer");
        assert!(key_name_for(&"x".repeat(500)).len() <= KEY_NAME_MAX);
    }

    #[test]
    fn authorize_url_carries_every_param_url_encoded() {
        let url = authorize_url("https://quome.studio/", "st", 51234, "ch", "quome-cli@a b");
        assert_eq!(
            url,
            "https://quome.studio/cli/authorize?kind=api_key&state=st&port=51234&code_challenge=ch&key_name=quome-cli%40a%20b"
        );
    }

    #[test]
    fn callback_parsing_needs_both_code_and_state() {
        assert_eq!(
            parse_callback("GET /callback?code=abc&state=xyz HTTP/1.1"),
            Some(("abc".into(), "xyz".into()))
        );
        assert_eq!(parse_callback("GET /callback?code=abc HTTP/1.1"), None);
        assert_eq!(parse_callback("GET /other?code=a&state=b HTTP/1.1"), None);
        assert_eq!(parse_callback("GET /favicon.ico HTTP/1.1"), None);
    }
}
