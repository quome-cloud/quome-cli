//! One line per event on stderr, as text (`models: event k=v`) or, with
//! `quome models serve --json-logs`, as one JSON object per line.
//!
//! Callers pass only ids, paths, codes, statuses and timings: never a token,
//! a header value, a body, a prompt or a completion.
use std::sync::atomic::{AtomicBool, Ordering};

static JSON: AtomicBool = AtomicBool::new(false);

pub fn set_json(on: bool) {
    JSON.store(on, Ordering::SeqCst);
}

fn json() -> bool {
    JSON.load(Ordering::SeqCst)
}

/// `models: <event> k=v …`, or `{"event": "<event>", "k": "v", …}`.
pub fn event(name: &str, fields: &[(&str, String)]) {
    eprintln!("{}", format_event(json(), name, fields));
}

/// A sentence for a person reading the log.
pub fn notice(message: &str) {
    eprintln!("{}", format_notice(json(), message));
}

/// One request line (see the proxy).
pub fn request(rid: &str, method: &str, path: &str, status: u16, ms: u128) {
    eprintln!("{}", format_request(json(), rid, method, path, status, ms));
}

fn format_event(json: bool, name: &str, fields: &[(&str, String)]) -> String {
    if json {
        let mut m = serde_json::Map::new();
        m.insert("event".into(), name.into());
        for (k, v) in fields {
            m.insert((*k).into(), v.clone().into());
        }
        serde_json::Value::Object(m).to_string()
    } else {
        let mut s = format!("models: {name}");
        for (k, v) in fields {
            s.push_str(&format!(" {k}={v}"));
        }
        s
    }
}

fn format_notice(json: bool, message: &str) -> String {
    if json {
        serde_json::json!({"event": "notice", "message": message}).to_string()
    } else {
        format!("models: {message}")
    }
}

fn format_request(
    json: bool,
    rid: &str,
    method: &str,
    path: &str,
    status: u16,
    ms: u128,
) -> String {
    if json {
        serde_json::json!({
            "event": "request", "rid": rid, "method": method, "path": path,
            "status": status, "ms": ms as u64,
        })
        .to_string()
    } else {
        format!("models: rid={rid} {method} {path} {status} {ms}ms")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_and_json_lines() {
        let f = [("code", "4403".to_string())];
        assert_eq!(
            format_event(false, "tunnel_closed", &f),
            "models: tunnel_closed code=4403"
        );
        assert_eq!(
            format_event(true, "tunnel_closed", &f),
            r#"{"event":"tunnel_closed","code":"4403"}"#
        );
        assert_eq!(format_notice(false, "Hi."), "models: Hi.");
        assert_eq!(
            format_request(false, "r-1", "POST", "/v1/chat/completions", 200, 12),
            "models: rid=r-1 POST /v1/chat/completions 200 12ms"
        );
        let v: serde_json::Value =
            serde_json::from_str(&format_request(true, "r-1", "GET", "/v1/models", 404, 3))
                .unwrap();
        assert_eq!(
            v,
            serde_json::json!({"event":"request","rid":"r-1","method":"GET","path":"/v1/models","status":404,"ms":3})
        );
    }
}
