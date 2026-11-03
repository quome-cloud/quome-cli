//! The loopback listener `quome connect` opens for the console's connect page.
//! Contract (quauth "Connect page"): `OPTIONS /callback` preflight, then one
//! `POST /callback` with the matching `state`, answered with a `state` echo.
use crate::errors::QuomeError;
use serde::Deserialize;
use std::io::Read;
use std::time::{Duration, Instant};
use tiny_http::{Header, Method, Request, Response, Server};

/// Largest accepted request body.
pub const MAX_BODY: usize = 64 * 1024;
/// How long `connect` waits for the browser.
pub const TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Callback {
    pub state: String,
    pub vrc: String,
    pub delegation_id: String,
    pub agent_principal: String,
    pub expires_at: String,
}

/// Longest a single request may take to arrive in full.
pub const REQUEST_DEADLINE: Duration = Duration::from_secs(5);

pub struct Listener {
    server: Server,
    port: u16,
    request_deadline: Duration,
}

/// `scheme://host[:port]` of a URL, the value of the CORS allow-origin header.
pub fn origin_of(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(u) => u.origin().ascii_serialization(),
        Err(_) => url.trim_end_matches('/').to_string(),
    }
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("valid header")
}

fn reply(req: Request, status: u16, origin: &str, body: Option<String>) {
    let mut resp = Response::from_string(body.unwrap_or_default()).with_status_code(status);
    resp.add_header(header("Access-Control-Allow-Origin", origin));
    resp.add_header(header("Content-Type", "application/json"));
    let _ = req.respond(resp);
}

impl Listener {
    /// Listen on 127.0.0.1 with an OS-assigned port.
    pub fn bind() -> Result<Listener, QuomeError> {
        let server = Server::http("127.0.0.1:0")
            .map_err(|e| QuomeError::CloudState(format!("cannot listen on loopback: {e}")))?;
        let port = server
            .server_addr()
            .to_ip()
            .map(|a| a.port())
            .ok_or_else(|| QuomeError::CloudState("cannot listen on loopback".into()))?;
        Ok(Listener {
            server,
            port,
            request_deadline: REQUEST_DEADLINE,
        })
    }

    pub fn with_request_deadline(mut self, d: Duration) -> Listener {
        self.request_deadline = d;
        self
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Serve until one good `POST /callback` arrives or `timeout` passes. Bad
    /// requests are refused and the wait goes on. Each request is handled on
    /// its own thread with a read deadline, so a stalled client cannot block
    /// the others or the overall timeout. Blocking.
    pub fn wait(
        &self,
        state: &str,
        issuer_origin: &str,
        timeout: Duration,
    ) -> Result<Callback, QuomeError> {
        let deadline = Instant::now() + timeout;
        let (tx, rx) = std::sync::mpsc::channel();
        loop {
            if let Ok(cb) = rx.try_recv() {
                return Ok(cb);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(timed_out());
            }
            match self
                .server
                .recv_timeout(left.min(Duration::from_millis(50)))
            {
                Ok(Some(req)) => {
                    let (tx, state, origin) =
                        (tx.clone(), state.to_string(), issuer_origin.to_string());
                    let limit = self.request_deadline;
                    std::thread::spawn(move || {
                        if let Some(cb) = handle(req, &state, &origin, Instant::now() + limit) {
                            let _ = tx.send(cb);
                        }
                    });
                }
                Ok(None) => {}
                Err(e) => return Err(QuomeError::CloudState(format!("loopback listener: {e}"))),
            }
        }
    }
}

fn timed_out() -> QuomeError {
    QuomeError::CloudState("timed out after 5 minutes waiting for the browser".into())
}

/// Answer one request; `Some` only for the accepted callback.
fn handle(mut req: Request, state: &str, origin: &str, by: Instant) -> Option<Callback> {
    let path = req.url().split('?').next().unwrap_or("").to_string();
    if path != "/callback" {
        reply(req, 404, origin, None);
        return None;
    }
    match req.method() {
        Method::Options => {
            let mut resp = Response::empty(204);
            for (k, v) in [
                ("Access-Control-Allow-Origin", origin),
                ("Access-Control-Allow-Methods", "POST"),
                ("Access-Control-Allow-Headers", "content-type"),
                ("Access-Control-Allow-Private-Network", "true"),
            ] {
                resp.add_header(header(k, v));
            }
            let _ = req.respond(resp);
            None
        }
        Method::Post => {
            let is_json = req
                .headers()
                .iter()
                .find(|h| h.field.equiv("Content-Type"))
                .is_some_and(|h| {
                    h.value
                        .as_str()
                        .to_ascii_lowercase()
                        .starts_with("application/json")
                });
            if !is_json {
                reply(req, 415, origin, None);
                return None;
            }
            if req.body_length().is_some_and(|n| n > MAX_BODY) {
                reply(req, 413, origin, None);
                return None;
            }
            let mut body = Vec::new();
            let read = req
                .as_reader()
                .take(MAX_BODY as u64 + 1)
                .read_to_end(&mut body);
            if Instant::now() > by {
                return None; // too slow: drop the request unanswered
            }
            if read.is_err() {
                reply(req, 400, origin, None);
                return None;
            }
            if body.len() > MAX_BODY {
                reply(req, 413, origin, None);
                return None;
            }
            let v: serde_json::Value = match serde_json::from_slice(&body) {
                Ok(v) => v,
                Err(_) => {
                    reply(req, 400, origin, None);
                    return None;
                }
            };
            if v["state"].as_str() != Some(state) {
                reply(req, 403, origin, None);
                return None;
            }
            match serde_json::from_value::<Callback>(v) {
                Ok(cb) => {
                    let echo = serde_json::json!({ "state": state }).to_string();
                    reply(req, 200, origin, Some(echo));
                    Some(cb)
                }
                Err(_) => {
                    reply(req, 400, origin, None);
                    None
                }
            }
        }
        _ => {
            reply(req, 405, origin, None);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGIN: &str = "https://auth.example.com";

    fn start(timeout: Duration) -> (u16, std::thread::JoinHandle<Result<Callback, QuomeError>>) {
        let l = Listener::bind().unwrap();
        let port = l.port();
        let h = std::thread::spawn(move || l.wait("abc123", ORIGIN, timeout));
        (port, h)
    }

    fn url(port: u16) -> String {
        format!("http://127.0.0.1:{port}/callback")
    }

    fn good() -> serde_json::Value {
        serde_json::json!({"state":"abc123","vrc":"V","delegation_id":"d1",
            "agent_principal":"p1","expires_at":"2027-01-01T00:00:00Z"})
    }

    #[tokio::test]
    async fn options_returns_the_four_cors_headers() {
        let (port, h) = start(Duration::from_secs(10));
        let c = reqwest::Client::new();
        let r = c
            .request(reqwest::Method::OPTIONS, url(port))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 204);
        let hv = |n: &str| r.headers().get(n).unwrap().to_str().unwrap().to_string();
        assert_eq!(hv("access-control-allow-origin"), ORIGIN);
        assert_eq!(hv("access-control-allow-methods"), "POST");
        assert_eq!(hv("access-control-allow-headers"), "content-type");
        assert_eq!(hv("access-control-allow-private-network"), "true");
        // still waiting: finish with a good POST
        c.post(url(port)).json(&good()).send().await.unwrap();
        h.join().unwrap().unwrap();
    }

    #[tokio::test]
    async fn good_post_echoes_state_with_acao() {
        let (port, h) = start(Duration::from_secs(10));
        let r = reqwest::Client::new()
            .post(url(port))
            .json(&good())
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        assert_eq!(r.headers()["access-control-allow-origin"], ORIGIN);
        let v: serde_json::Value = r.json().await.unwrap();
        assert_eq!(v, serde_json::json!({"state":"abc123"}));
        let cb = h.join().unwrap().unwrap();
        assert_eq!(cb.delegation_id, "d1");
        assert_eq!(cb.vrc, "V");
    }

    #[tokio::test]
    async fn bad_requests_are_refused_and_listening_continues() {
        let (port, h) = start(Duration::from_secs(10));
        let c = reqwest::Client::new();
        let mut wrong = good();
        wrong["state"] = "nope".into();
        assert_eq!(
            c.post(url(port))
                .json(&wrong)
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
        assert_eq!(c.get(url(port)).send().await.unwrap().status(), 405);
        assert_eq!(c.put(url(port)).send().await.unwrap().status(), 405);
        let other = format!("http://127.0.0.1:{port}/other");
        assert_eq!(
            c.post(other).json(&good()).send().await.unwrap().status(),
            404
        );
        let big = serde_json::json!({"state":"abc123","vrc":"x".repeat(70 * 1024)});
        assert_eq!(
            c.post(url(port)).json(&big).send().await.unwrap().status(),
            413
        );
        let r = c
            .post(url(port))
            .body("not json")
            .header("content-type", "application/json")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 400);
        assert!(!h.is_finished());
        assert_eq!(
            c.post(url(port))
                .json(&good())
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
        h.join().unwrap().unwrap();
    }

    #[test]
    fn times_out_when_nobody_calls() {
        let (_port, h) = start(Duration::from_millis(200));
        assert!(h.join().unwrap().is_err());
    }

    #[test]
    fn origin_drops_path_and_keeps_port() {
        assert_eq!(
            origin_of("https://auth.quome.cloud/x"),
            "https://auth.quome.cloud"
        );
        assert_eq!(origin_of("http://127.0.0.1:8080"), "http://127.0.0.1:8080");
    }

    fn stalled(port: u16, declared: usize, sent: &str) -> std::net::TcpStream {
        use std::io::Write;
        let mut c = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        c.set_nodelay(true).unwrap();
        let head = format!(
            "POST /callback HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: {declared}\r\n\r\n{sent}"
        );
        c.write_all(head.as_bytes()).unwrap();
        c.flush().unwrap();
        c
    }

    #[tokio::test]
    async fn a_stalled_request_does_not_wedge_the_listener() {
        let (port, h) = start(Duration::from_secs(10));
        // over 1024 bytes tiny_http leaves the body to us, so this is the wedging case
        let _stall = stalled(port, 5000, "{\"state\":");
        tokio::time::sleep(Duration::from_millis(100)).await;
        let r = reqwest::Client::new()
            .post(url(port))
            .json(&good())
            .timeout(Duration::from_secs(3))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        assert_eq!(h.join().unwrap().unwrap().delegation_id, "d1");
    }

    #[tokio::test]
    async fn a_request_finishing_after_its_deadline_is_dropped() {
        use std::io::Write;
        let l = Listener::bind()
            .unwrap()
            .with_request_deadline(Duration::from_millis(300));
        let port = l.port();
        let h = std::thread::spawn(move || l.wait("abc123", ORIGIN, Duration::from_secs(10)));
        let late =
            serde_json::json!({"state":"abc123","vrc":"V".repeat(2000),"delegation_id":"late",
            "agent_principal":"p","expires_at":"e"})
            .to_string();
        let (a, b) = late.split_at(10);
        let mut s = stalled(port, late.len(), a);
        tokio::time::sleep(Duration::from_millis(700)).await;
        s.write_all(b.as_bytes()).unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        if h.is_finished() {
            panic!("finished: {:?}", h.join().unwrap());
        }
        let r = reqwest::Client::new()
            .post(url(port))
            .json(&good())
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        assert_eq!(h.join().unwrap().unwrap().delegation_id, "d1");
    }
}
