//! One gateway request per tunnel stream: verify the gateway's request token,
//! apply the path and concurrency rules, then stream the request to the local
//! model server and its answer back, chunk by chunk, never buffered.
//!
//! Logs carry the request id, method, path, status and milliseconds only:
//! never a token, a header value, a body, a prompt or a completion.
use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::StreamExt;
use http::header::{self, HeaderMap, HeaderName, HeaderValue};
use http::{Method, StatusCode};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, BodyStream, Full, StreamBody};
use hyper::body::{Body as _, Frame, Incoming};
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite};

use super::protocol::{path_allowed, RequestTokenVerifier};

/// The largest request body forwarded to the local server.
pub const MAX_BODY_BYTES: u64 = 16 * 1024 * 1024;
pub const TOKEN_HEADER: &str = "x-quome-request-token";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Logged when the caller goes away before the local server answers.
const CALLER_HUNG_UP: StatusCode = match StatusCode::from_u16(499) {
    Ok(s) => s,
    Err(_) => StatusCode::BAD_GATEWAY,
};
/// A stream that has not sent its whole request head by then is closed.
pub const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(30);
const RESPONSES_UNSUPPORTED: &str =
    "This machine's Ollama is older than 0.13.3; upgrade Ollama to serve /v1/responses.";

type BoxError = Box<dyn std::error::Error + Send + Sync>;
type ProxyBody = BoxBody<Bytes, BoxError>;

pub struct ProxyState {
    /// `None` until the gateway's `welcome` names its key.
    pub verifier: tokio::sync::Mutex<Option<RequestTokenVerifier>>,
    pub local: url::Url,
    /// `"Bearer …"` from `api_key_file`, set on every forwarded request.
    pub local_auth: Option<String>,
    /// See [`local_client`].
    pub client: reqwest::Client,
    /// `max_concurrency` permits.
    pub slots: tokio::sync::Semaphore,
    pub responses_supported: AtomicBool,
    pub clock: fn() -> i64,
}

/// The client for the local server: never through a proxy, a 2 s connect
/// timeout and no total timeout (a generation may stream for minutes).
pub fn local_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .unwrap_or_default()
}

/// Serve exactly one HTTP/1.1 request on `io` (keep-alive off), then return.
pub async fn serve_stream<IO: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    io: IO,
    st: Arc<ProxyState>,
) {
    let svc = hyper::service::service_fn(move |req| {
        let st = st.clone();
        async move { Ok::<_, Infallible>(handle(req, st).await) }
    });
    let _ = hyper::server::conn::http1::Builder::new()
        .keep_alive(false)
        .timer(hyper_util::rt::TokioTimer::new())
        .header_read_timeout(HEADER_READ_TIMEOUT)
        .serve_connection(TokioIo::new(io), svc)
        .await;
}

/// What one log line says about a request.
struct Line {
    rid: String,
    method: Method,
    path: String,
    started: Instant,
}

impl Line {
    fn log(&self, status: StatusCode) {
        super::log::request(
            &self.rid,
            self.method.as_str(),
            &self.path,
            status.as_u16(),
            self.started.elapsed().as_millis(),
        );
    }

    fn answer(&self, status: StatusCode, body: serde_json::Value) -> Response<ProxyBody> {
        self.log(status);
        json_response(status, body)
    }
}

fn refused(reason: &str) -> serde_json::Value {
    serde_json::json!({"error": {"code": "request_token_refused", "reason": reason}})
}

fn code(code: &str) -> serde_json::Value {
    serde_json::json!({"error": {"code": code}})
}

/// A held concurrency permit plus the request's log line. It lives as long as
/// the response body: dropping it (body finished, or the caller hung up)
/// returns the permit and writes the log line.
struct InFlight {
    st: Arc<ProxyState>,
    line: Line,
    status: StatusCode,
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.st.slots.add_permits(1);
        self.line.log(self.status);
    }
}

fn hop_by_hop(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection" | "keep-alive" | "te" | "trailer" | "transfer-encoding" | "upgrade"
    ) || name.as_str().starts_with("proxy-")
}

fn forwarded_request_header(name: &HeaderName) -> bool {
    !(hop_by_hop(name)
        || name == header::HOST
        || name == header::AUTHORIZATION
        || name == header::CONTENT_LENGTH
        || name.as_str().starts_with("x-quome-"))
}

async fn handle(req: Request<Incoming>, st: Arc<ProxyState>) -> Response<ProxyBody> {
    let mut line = Line {
        rid: "-".into(),
        method: req.method().clone(),
        path: req.uri().path().to_owned(),
        started: Instant::now(),
    };

    let verdict = {
        let mut guard = st.verifier.lock().await;
        match guard.as_mut() {
            None => Err("no_gateway_key"),
            Some(v) => match req.headers().get(TOKEN_HEADER) {
                None => Err("missing"),
                Some(t) => match t.to_str() {
                    Err(_) => Err("malformed"),
                    Ok(t) => v.verify(t, (st.clock)()).map_err(|r| r.as_str()),
                },
            },
        }
    };
    match verdict {
        Ok(claims) => line.rid = claims.rid,
        Err(reason) => return line.answer(StatusCode::BAD_GATEWAY, refused(reason)),
    }

    if !path_allowed(&line.method, &line.path) {
        return line.answer(StatusCode::NOT_FOUND, code("path_not_allowed"));
    }
    if line.method == Method::POST
        && line.path == "/v1/responses"
        && !st.responses_supported.load(Ordering::SeqCst)
    {
        return line.answer(
            StatusCode::NOT_FOUND,
            serde_json::json!({"error": {"code": "responses_unsupported", "message": RESPONSES_UNSUPPORTED}}),
        );
    }

    let Ok(permit) = st.slots.try_acquire() else {
        return line.answer(StatusCode::TOO_MANY_REQUESTS, code("machine_busy"));
    };
    permit.forget();
    // From here every return path drops `slot`, which gives the permit back.
    // Until the local server answers, a drop means the caller hung up (499).
    let mut slot = InFlight {
        st: st.clone(),
        line,
        status: CALLER_HUNG_UP,
    };

    let declared = match req.headers().get(header::CONTENT_LENGTH) {
        None => None,
        Some(v) => match v.to_str().ok().and_then(|s| s.parse::<u64>().ok()) {
            Some(n) => Some(n),
            None => return finish(slot, StatusCode::BAD_REQUEST, code("bad_content_length")),
        },
    };
    if declared.is_some_and(|n| n > MAX_BODY_BYTES) {
        return finish(
            slot,
            StatusCode::PAYLOAD_TOO_LARGE,
            code("request_too_large"),
        );
    }

    let (parts, body) = req.into_parts();
    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or(&slot.line.path);
    let url = format!(
        "{}{}",
        st.local.as_str().trim_end_matches('/'),
        path_and_query
    );

    let mut headers = HeaderMap::new();
    for (name, value) in parts.headers.iter() {
        if forwarded_request_header(name) {
            headers.append(name.clone(), value.clone());
        }
    }
    if let Some(auth) = &st.local_auth {
        match HeaderValue::from_str(auth) {
            Ok(mut v) => {
                v.set_sensitive(true);
                headers.insert(header::AUTHORIZATION, v);
            }
            Err(_) => {
                return finish(
                    slot,
                    StatusCode::BAD_GATEWAY,
                    code("local_server_unreachable"),
                )
            }
        }
    }

    let over_cap = Arc::new(AtomicBool::new(false));
    let mut out = st
        .client
        .request(parts.method.clone(), url)
        .headers(headers);
    if declared == Some(0) || (declared.is_none() && body.is_end_stream()) {
        // No body: send none, rather than an empty chunked one.
    } else {
        if let Some(n) = declared {
            out = out.header(header::CONTENT_LENGTH, n);
        }
        out = out.body(reqwest::Body::wrap_stream(capped(body, over_cap.clone())));
    }

    let resp = match out.send().await {
        Ok(r) => r,
        Err(_) if over_cap.load(Ordering::SeqCst) => {
            return finish(
                slot,
                StatusCode::PAYLOAD_TOO_LARGE,
                code("request_too_large"),
            )
        }
        Err(_) => {
            return finish(
                slot,
                StatusCode::BAD_GATEWAY,
                code("local_server_unreachable"),
            )
        }
    };

    let status = resp.status();
    slot.status = status;
    let mut answer = Response::builder().status(status);
    if let Some(h) = answer.headers_mut() {
        for (name, value) in resp.headers().iter() {
            if !hop_by_hop(name) {
                h.append(name.clone(), value.clone());
            }
        }
    }
    // The permit and the log line ride in the body: they end when the body
    // does, and a caller hang-up drops the body, which drops the local
    // response and closes its connection.
    let frames = resp.bytes_stream().map(move |chunk| {
        let _held = &slot;
        chunk.map(Frame::data).map_err(BoxError::from)
    });
    answer
        .body(BodyExt::boxed(StreamBody::new(frames)))
        .unwrap_or_else(|_| Response::new(Full::new(Bytes::new()).map_err(|n| match n {}).boxed()))
}

/// An error answer after the permit was taken: the permit goes back and the
/// line is logged with this status when `slot` drops.
fn finish(mut slot: InFlight, status: StatusCode, body: serde_json::Value) -> Response<ProxyBody> {
    slot.status = status;
    json_response(status, body)
}

fn json_response(status: StatusCode, body: serde_json::Value) -> Response<ProxyBody> {
    let mut r = Response::new(
        Full::new(Bytes::from(body.to_string()))
            .map_err(|never| match never {})
            .boxed(),
    );
    *r.status_mut() = status;
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    r
}

/// The caller's body as a stream of data chunks that fails, and flags
/// `over_cap`, once more than [`MAX_BODY_BYTES`] have passed.
fn capped(
    body: Incoming,
    over_cap: Arc<AtomicBool>,
) -> impl futures_util::Stream<Item = Result<Bytes, BoxError>> + Send + 'static {
    let mut seen: u64 = 0;
    BodyStream::new(body).filter_map(move |frame| {
        let out = match frame {
            Err(e) => Some(Err(BoxError::from(e))),
            Ok(f) => match f.into_data() {
                Err(_) => None, // trailers are not forwarded
                Ok(data) => {
                    seen += data.len() as u64;
                    if seen > MAX_BODY_BYTES {
                        over_cap.store(true, Ordering::SeqCst);
                        Some(Err(BoxError::from("request body over the cap")))
                    } else {
                        Some(Ok(data))
                    }
                }
            },
        };
        std::future::ready(out)
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::machine::protocol::GatewayKey;
    use ed25519_dalek::{Signer, SigningKey};
    use http_body_util::{BodyExt, Full};
    use hyper::body::Incoming;
    use serde_json::{json, Value};
    use std::time::{Duration, Instant};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::oneshot;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const VECTORS: &str = include_str!("../../tests/vectors/models-tunnel-v1.json");
    const NOW: i64 = 1_767_225_600;

    pub(crate) fn fixed_now() -> i64 {
        NOW
    }

    pub(crate) fn vec_str(k: &str) -> String {
        let v: Value = serde_json::from_str(VECTORS).unwrap();
        v["request_token"][k].as_str().unwrap().to_owned()
    }

    fn signer() -> SigningKey {
        let hex = vec_str("gateway_seed_hex");
        let seed: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        SigningKey::from_bytes(&seed.try_into().unwrap())
    }

    fn b64(b: &[u8]) -> String {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use base64::Engine;
        URL_SAFE_NO_PAD.encode(b)
    }

    fn token(rid: &str, exp: i64) -> String {
        let h = b64(br#"{"alg":"EdDSA","typ":"JWT"}"#);
        let claims =
            json!({"aud": vec_str("audience"), "rid": rid, "model": "qwen3:32b", "exp": exp});
        let p = b64(claims.to_string().as_bytes());
        let sig = signer().sign(format!("{h}.{p}").as_bytes());
        format!("{h}.{p}.{}", b64(&sig.to_bytes()))
    }

    pub(crate) fn good(rid: &str) -> String {
        token(rid, NOW + 60)
    }

    fn verifier() -> RequestTokenVerifier {
        let key = GatewayKey::from_b64url(&b64(signer().verifying_key().as_bytes())).unwrap();
        RequestTokenVerifier::new(key, vec_str("audience"))
    }

    fn state(local: &str, auth: Option<&str>, max: usize, responses: bool) -> Arc<ProxyState> {
        Arc::new(ProxyState {
            verifier: tokio::sync::Mutex::new(Some(verifier())),
            local: url::Url::parse(local).unwrap(),
            local_auth: auth.map(str::to_owned),
            client: local_client(),
            slots: tokio::sync::Semaphore::new(max),
            responses_supported: AtomicBool::new(responses),
            clock: fixed_now,
        })
    }

    fn req(
        m: &str,
        p: &str,
        tok: Option<&str>,
        body: &'static [u8],
    ) -> hyper::Request<Full<Bytes>> {
        let mut b = hyper::Request::builder()
            .method(m)
            .uri(p)
            .header("host", "machine");
        if let Some(t) = tok {
            b = b.header("x-quome-request-token", t);
        }
        b.body(Full::new(Bytes::from_static(body))).unwrap()
    }

    /// One stream: the proxy on one end of a duplex, a hyper client on the other.
    async fn open(
        st: Arc<ProxyState>,
        r: hyper::Request<Full<Bytes>>,
    ) -> (hyper::Response<Incoming>, tokio::task::JoinHandle<()>) {
        let (a, b) = tokio::io::duplex(64 * 1024);
        tokio::spawn(serve_stream(b, st));
        let (mut tx, conn) = hyper::client::conn::http1::handshake(TokioIo::new(a))
            .await
            .unwrap();
        let conn = tokio::spawn(async move {
            let _ = conn.await;
        });
        let resp = tokio::time::timeout(Duration::from_secs(10), tx.send_request(r))
            .await
            .expect("the proxy answers within 10 s")
            .unwrap();
        (resp, conn)
    }

    async fn send(st: Arc<ProxyState>, r: hyper::Request<Full<Bytes>>) -> (u16, Vec<u8>) {
        let (resp, _conn) = open(st, r).await;
        let status = resp.status().as_u16();
        let body = resp
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec();
        (status, body)
    }

    fn as_json(b: &[u8]) -> Value {
        serde_json::from_slice(b).unwrap()
    }

    async fn chat_mock(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true}"#))
            .mount(server)
            .await;
    }

    pub(crate) const CHAT: &[u8] =
        br#"{"model":"qwen3:32b","messages":[{"role":"user","content":"hi"}]}"#;

    #[tokio::test]
    async fn forwards_chat_unchanged() {
        let local = MockServer::start().await;
        chat_mock(&local).await;
        let st = state(&local.uri(), None, 4, true);
        let (status, body) = send(
            st,
            req("POST", "/v1/chat/completions?x=1", Some(&good("r-1")), CHAT),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(body, br#"{"ok":true}"#);
        let seen = local.received_requests().await.unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].body, CHAT);
        assert_eq!(seen[0].url.query(), Some("x=1"));
        assert!(seen[0].headers.get("x-quome-request-token").is_none());
        assert!(seen[0].headers.get("authorization").is_none());
    }

    #[tokio::test]
    async fn injects_the_local_key_only() {
        let local = MockServer::start().await;
        chat_mock(&local).await;
        let st = state(&local.uri(), Some("Bearer local-k"), 4, true);
        let mut r = req("POST", "/v1/chat/completions", Some(&good("r-1")), CHAT);
        r.headers_mut()
            .insert("authorization", "Bearer caller".parse().unwrap());
        let (status, _) = send(st, r).await;
        assert_eq!(status, 200);
        let seen = local.received_requests().await.unwrap();
        let auth: Vec<_> = seen[0].headers.get_all("authorization").iter().collect();
        assert_eq!(auth, vec!["Bearer local-k"]);
    }

    #[tokio::test]
    async fn refuses_before_welcome() {
        let local = MockServer::start().await;
        let st = state(&local.uri(), None, 4, true);
        *st.verifier.lock().await = None;
        let (status, body) = send(
            st,
            req("POST", "/v1/chat/completions", Some(&good("r-1")), CHAT),
        )
        .await;
        assert_eq!(status, 502);
        assert_eq!(
            as_json(&body),
            json!({"error":{"code":"request_token_refused","reason":"no_gateway_key"}})
        );
        assert!(local.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn refuses_bad_token() {
        let local = MockServer::start().await;
        let st = state(&local.uri(), None, 4, true);
        let expired = token("r-1", NOW - 31);
        let (status, body) = send(
            st.clone(),
            req("POST", "/v1/chat/completions", Some(&expired), CHAT),
        )
        .await;
        assert_eq!(status, 502);
        assert_eq!(
            as_json(&body),
            json!({"error":{"code":"request_token_refused","reason":"expired"}})
        );
        let (status, body) = send(st, req("POST", "/v1/chat/completions", None, CHAT)).await;
        assert_eq!(status, 502);
        assert_eq!(as_json(&body)["error"]["code"], "request_token_refused");
        assert!(local.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn refuses_replay() {
        let local = MockServer::start().await;
        chat_mock(&local).await;
        let st = state(&local.uri(), None, 4, true);
        let t = good("r-1");
        let (first, _) = send(
            st.clone(),
            req("POST", "/v1/chat/completions", Some(&t), CHAT),
        )
        .await;
        assert_eq!(first, 200);
        let (status, body) = send(st, req("POST", "/v1/chat/completions", Some(&t), CHAT)).await;
        assert_eq!(status, 502);
        assert_eq!(
            as_json(&body),
            json!({"error":{"code":"request_token_refused","reason":"replayed"}})
        );
        assert_eq!(local.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn refuses_other_paths() {
        let local = MockServer::start().await;
        let st = state(&local.uri(), None, 4, true);
        let (status, _) = send(st.clone(), req("GET", "/api/tags", Some(&good("r-1")), b"")).await;
        assert_eq!(status, 404);
        let (status, _) = send(
            st,
            req("GET", "/v1/models/../api/tags", Some(&good("r-2")), b""),
        )
        .await;
        assert_eq!(status, 404);
        assert!(local.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn forwards_models_listing_without_a_body() {
        let local = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"data":[]}"#))
            .mount(&local)
            .await;
        let st = state(&local.uri(), None, 4, true);
        let (status, body) = send(st, req("GET", "/v1/models", Some(&good("r-1")), b"")).await;
        assert_eq!(status, 200);
        assert_eq!(body, br#"{"data":[]}"#);
        let seen = local.received_requests().await.unwrap();
        assert!(seen[0].body.is_empty());
        assert!(seen[0].headers.get("transfer-encoding").is_none());
    }

    #[tokio::test]
    async fn responses_gate() {
        let local = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&local)
            .await;
        let st = state(&local.uri(), None, 4, false);
        let (status, body) = send(
            st.clone(),
            req("POST", "/v1/responses", Some(&good("r-1")), b"{}"),
        )
        .await;
        assert_eq!(status, 404);
        assert_eq!(
            as_json(&body),
            json!({"error":{"code":"responses_unsupported","message":"This machine's Ollama is older than 0.13.3; upgrade Ollama to serve /v1/responses."}})
        );
        assert!(local.received_requests().await.unwrap().is_empty());
        st.responses_supported.store(true, Ordering::SeqCst);
        let (status, _) = send(st, req("POST", "/v1/responses", Some(&good("r-2")), b"{}")).await;
        assert_eq!(status, 200);
        assert_eq!(local.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn rejects_a_body_over_the_cap() {
        let local = MockServer::start().await;
        let st = state(&local.uri(), None, 4, true);
        let mut r = req("POST", "/v1/chat/completions", Some(&good("r-1")), b"");
        r.headers_mut().insert(
            "content-length",
            (MAX_BODY_BYTES + 1).to_string().parse().unwrap(),
        );
        let (resp, _conn) = open(st, r).await;
        assert_eq!(resp.status().as_u16(), 413);
        assert!(local.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn rejects_a_streamed_body_over_the_cap() {
        let local = MockServer::start().await;
        chat_mock(&local).await;
        let st = state(&local.uri(), None, 4, true);
        let mib = Bytes::from(vec![b'x'; 1024 * 1024]);
        let chunks = (0..17).map(move |_| Ok::<_, Infallible>(Frame::data(mib.clone())));
        let r = hyper::Request::post("/v1/chat/completions")
            .header("x-quome-request-token", good("r-1"))
            .body(StreamBody::new(futures_util::stream::iter(chunks)))
            .unwrap();
        let (a, b) = tokio::io::duplex(64 * 1024);
        tokio::spawn(serve_stream(b, st.clone()));
        let (mut tx, conn) = hyper::client::conn::http1::handshake(TokioIo::new(a))
            .await
            .unwrap();
        tokio::spawn(conn);
        let resp = tokio::time::timeout(Duration::from_secs(10), tx.send_request(r))
            .await
            .expect("the proxy answers within 10 s")
            .unwrap();
        assert_eq!(resp.status().as_u16(), 413);
        assert!(local.received_requests().await.unwrap().is_empty());
        assert_eq!(st.slots.available_permits(), 4);
    }

    /// A raw local server the test drives byte by byte.
    async fn raw_local() -> (String, TcpListener) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        (format!("http://{}", l.local_addr().unwrap()), l)
    }

    async fn read_head(s: &mut TcpStream) {
        let mut buf = Vec::new();
        let mut b = [0u8; 1024];
        while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = s.read(&mut b).await.unwrap();
            assert!(n > 0, "local closed before the request head");
            buf.extend_from_slice(&b[..n]);
        }
    }

    const SSE_HEAD: &[u8] =
        b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n";

    fn chunk(data: &[u8]) -> Vec<u8> {
        let mut v = format!("{:x}\r\n", data.len()).into_bytes();
        v.extend_from_slice(data);
        v.extend_from_slice(b"\r\n");
        v
    }

    #[tokio::test]
    async fn over_concurrency_is_429() {
        let (url, l) = raw_local().await;
        let (release_tx, release_rx) = oneshot::channel::<()>();
        let (seen_tx, seen_rx) = oneshot::channel::<()>();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            read_head(&mut s).await;
            let _ = seen_tx.send(());
            let _ = release_rx.await;
            s.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}")
                .await
                .unwrap();
        });
        let st = state(&url, None, 1, true);
        let first = tokio::spawn(send(
            st.clone(),
            req("GET", "/v1/models", Some(&good("r-1")), b""),
        ));
        tokio::time::timeout(Duration::from_secs(5), seen_rx)
            .await
            .expect("the first request reaches the local server")
            .unwrap();
        let (status, _) = send(
            st.clone(),
            req("GET", "/v1/models", Some(&good("r-2")), b""),
        )
        .await;
        assert_eq!(status, 429);
        release_tx.send(()).unwrap();
        assert_eq!(first.await.unwrap().0, 200);
        assert_eq!(st.slots.available_permits(), 1);
    }

    #[tokio::test]
    async fn local_down_is_502_fast() {
        let (url, l) = raw_local().await;
        drop(l);
        let st = state(&url, None, 4, true);
        let t0 = Instant::now();
        let (status, body) = send(st, req("GET", "/v1/models", Some(&good("r-1")), b"")).await;
        assert!(t0.elapsed() < Duration::from_millis(2500));
        assert_eq!(status, 502);
        assert_eq!(
            as_json(&body),
            json!({"error":{"code":"local_server_unreachable"}})
        );
    }

    #[tokio::test]
    async fn streams_without_buffering() {
        let (url, l) = raw_local().await;
        let (release_tx, release_rx) = oneshot::channel::<()>();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            read_head(&mut s).await;
            s.write_all(SSE_HEAD).await.unwrap();
            s.write_all(&chunk(b"data: {\"a\":1}\n\n")).await.unwrap();
            let _ = release_rx.await;
            s.write_all(&chunk(b"data: [DONE]\n\n")).await.unwrap();
            s.write_all(b"0\r\n\r\n").await.unwrap();
        });
        let st = state(&url, None, 4, true);
        let (resp, _conn) = open(
            st,
            req("POST", "/v1/chat/completions", Some(&good("r-1")), CHAT),
        )
        .await;
        assert_eq!(resp.status().as_u16(), 200);
        assert_eq!(resp.headers()["content-type"], "text/event-stream");
        let mut body = resp.into_body();
        let first = tokio::time::timeout(Duration::from_secs(2), body.frame())
            .await
            .expect("first chunk must arrive before the local server finishes")
            .unwrap()
            .unwrap();
        assert_eq!(first.into_data().unwrap(), &b"data: {\"a\":1}\n\n"[..]);
        release_tx.send(()).unwrap();
        let rest = tokio::time::timeout(Duration::from_secs(5), body.collect())
            .await
            .expect("the rest arrives once released")
            .unwrap()
            .to_bytes();
        assert_eq!(rest, &b"data: [DONE]\n\n"[..]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_stream_without_a_request_head_is_closed() {
        // Nothing at all, then half a head.
        for sent in [&b""[..], b"POST /v1/chat"] {
            let st = state("http://127.0.0.1:9", None, 4, true);
            let (mut a, b) = tokio::io::duplex(64 * 1024);
            let t0 = tokio::time::Instant::now();
            let served = tokio::spawn(serve_stream(b, st));
            a.write_all(sent).await.unwrap();
            tokio::time::timeout(Duration::from_secs(120), served)
                .await
                .expect("an idle stream is closed, not kept forever")
                .unwrap();
            assert!(t0.elapsed() >= HEADER_READ_TIMEOUT);
        }
    }

    #[tokio::test]
    async fn caller_hangup_drops_local() {
        let (url, l) = raw_local().await;
        let (closed_tx, closed_rx) = oneshot::channel::<Instant>();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            read_head(&mut s).await;
            s.write_all(SSE_HEAD).await.unwrap();
            let (mut rd, mut wr) = s.into_split();
            let writer = async move {
                loop {
                    if wr.write_all(&chunk(b"data: {}\n\n")).await.is_err() {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            };
            let reader = async move {
                let mut b = [0u8; 64];
                loop {
                    match rd.read(&mut b).await {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {}
                    }
                }
            };
            tokio::select! {
                _ = writer => {}
                _ = reader => {}
            }
            let _ = closed_tx.send(Instant::now());
        });
        let st = state(&url, None, 1, true);
        let (resp, conn) = open(
            st.clone(),
            req("POST", "/v1/chat/completions", Some(&good("r-1")), CHAT),
        )
        .await;
        let mut body = resp.into_body();
        body.frame().await.unwrap().unwrap();
        let hung_up = Instant::now();
        drop(body);
        conn.abort();
        let closed = tokio::time::timeout(Duration::from_secs(1), closed_rx)
            .await
            .expect("the local connection must close within 1 s of the caller hanging up")
            .unwrap();
        assert!(closed.duration_since(hung_up) < Duration::from_secs(1));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(st.slots.available_permits(), 1, "the slot comes back");
    }
}
