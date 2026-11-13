//! The machine's one tunnel to the gateway: an outbound WebSocket whose binary
//! messages carry yamux. The machine opens the control stream first (hello,
//! adverts, fresh tokens; the gateway answers welcome); the gateway opens one
//! stream per request, each served by the proxy.
//!
//! Transient failures never end [`run`]: it reconnects with jittered backoff.
//! Only a revoked delegation, a missing capability, an upgraded binary or a
//! stop signal end it. Logs never carry a token.
use std::future::poll_fn;
use std::path::PathBuf;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use futures_util::future::BoxFuture;
use futures_util::{SinkExt, StreamExt};
use http::header::{HeaderValue, AUTHORIZATION};
use rand_core::RngCore;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::io::{DuplexStream, WriteHalf};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{sleep, sleep_until, Instant};
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;
use tokio_util::compat::{Compat, FuturesAsyncReadCompatExt, TokioAsyncReadCompatExt};

use super::config::MAX_CONCURRENCY;
use super::log;
use super::protocol::RequestTokenVerifier;
use super::protocol::{decode_line, encode_line, Advert, ControlMsg, GatewayKey};
use super::proxy::{serve_stream, ProxyState};
use crate::cloud::api::ModelsToken;
use crate::errors::QuomeError;

/// Each side's end of the WebSocket ⇄ yamux pipe.
const PIPE_BYTES: usize = 256 * 1024;
const DIAL_TIMEOUT: Duration = Duration::from_secs(20);
/// A failed token refresh is retried this often until the old token lapses.
const REFRESH_RETRY: Duration = Duration::from_secs(10);
/// A token answer without a lifetime counts as the platform's longest (300 s).
const DEFAULT_TOKEN_SECS: u64 = 300;
/// How long a closing tunnel waits for the WebSocket to say why, or to close cleanly.
const CLOSE_GRACE: Duration = Duration::from_secs(1);
const CLOSE_BAD_TOKEN: u16 = 4401;
const CLOSE_NOT_ACTIVE: u16 = 4403;
const NOT_ACTIVE: &str =
    "This machine is not active on the platform (still being approved, revoked, or draining)";
/// yamux's default stream limit; the gateway never needs more than `max_concurrency`.
const YAMUX_MAX_STREAMS: usize = 512;
const _: () = assert!((MAX_CONCURRENCY as usize) < YAMUX_MAX_STREAMS);

pub type TokenFn =
    Arc<dyn Fn() -> BoxFuture<'static, Result<ModelsToken, QuomeError>> + Send + Sync>;
pub type AdvertFn = Arc<dyn Fn() -> BoxFuture<'static, Advert> + Send + Sync>;

#[derive(Clone, Debug)]
pub struct Timing {
    pub ping_every: Duration,
    pub dead_after: Duration,
    pub advert_every: Duration,
    pub refresh_before: Duration,
    pub backoff_min: Duration,
    pub backoff_max: Duration,
    /// A tunnel that lived this long resets the backoff.
    pub healthy_after: Duration,
    /// How often the program's path is checked for an upgrade.
    pub upgrade_every: Duration,
    /// How long a stop waits for requests in flight.
    pub drain: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            ping_every: Duration::from_secs(20),
            dead_after: Duration::from_secs(60),
            advert_every: Duration::from_secs(30),
            refresh_before: Duration::from_secs(60),
            backoff_min: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
            healthy_after: Duration::from_secs(60),
            upgrade_every: Duration::from_secs(600),
            drain: Duration::from_secs(10),
        }
    }
}

pub struct TunnelDeps {
    pub token: TokenFn,
    /// Also keeps `ProxyState::responses_supported` current.
    pub advert: AdvertFn,
    /// The machine's identity (`act.sub`): the request tokens' audience.
    pub audience: String,
    /// `wss://models.<apps_domain>/<org_slug>/machine`, used when the token
    /// response names no URL.
    pub fallback_url: String,
    pub timing: Timing,
    /// The program path this process was started from; when what it resolves
    /// to changes, the binary was upgraded. `None` never checks.
    pub program: Option<PathBuf>,
    /// Becomes `true` on SIGTERM / SIGINT.
    pub shutdown: watch::Receiver<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// The delegation is gone (disconnected, revoked or expired).
    Revoked,
    /// The delegation lacks `models_serve`.
    MissingCapability,
    /// The program on disk changed; restart into it.
    Upgraded,
    /// A stop signal; requests in flight were given time to finish.
    Shutdown,
}

/// Runs until a permanent stop; transient failures reconnect with jittered backoff.
pub async fn run(deps: TunnelDeps, proxy: Arc<ProxyState>) -> Stop {
    let mut deps = deps;
    let program = deps.program.clone().map(Program::new);
    let mut backoff = Backoff::new(deps.timing.backoff_min, deps.timing.backoff_max);
    let mut shutdown = deps.shutdown.clone();
    let mut retried_at_once = false;
    loop {
        if *shutdown.borrow() {
            return Stop::Shutdown;
        }
        if program.as_ref().is_some_and(Program::changed) {
            return Stop::Upgraded;
        }
        let fetched = tokio::select! {
            t = (deps.token)() => t,
            _ = stopped(&mut shutdown) => return Stop::Shutdown,
        };
        let out = match fetched {
            Err(e) => {
                if let Some(stop) = permanent(&e) {
                    return stop;
                }
                log::event("token_failed", &[("error", e.to_string())]);
                Outcome::failed()
            }
            Ok(token) => match tunnel_url(token.url.as_deref(), &deps.fallback_url) {
                Err(e) => {
                    log::event("tunnel_url_refused", &[("error", e.to_string())]);
                    Outcome::failed()
                }
                Ok(url) => connection(&mut deps, &proxy, program.as_ref(), token, url).await,
            },
        };
        *proxy.verifier.lock().await = None;
        let code = match out.end {
            End::Shutdown => return Stop::Shutdown,
            End::Upgraded => return Stop::Upgraded,
            End::Failed => None,
            End::Closed { code, reason } => {
                let mut f = vec![(
                    "code",
                    code.map_or_else(|| "none".to_string(), |c| c.to_string()),
                )];
                if code.is_none() {
                    f.push(("reason", reason.to_string()));
                }
                log::event("tunnel_closed", &f);
                if code == Some(CLOSE_NOT_ACTIVE) {
                    log::notice(NOT_ACTIVE);
                }
                code
            }
        };
        backoff.tunnel_ended(out.lived, deps.timing.healthy_after);
        // A refused token is refetched at once, but only once in a row.
        if code == Some(CLOSE_BAD_TOKEN) && !retried_at_once {
            retried_at_once = true;
            continue;
        }
        retried_at_once = false;
        let wait = backoff.next(rand_core::OsRng.next_u64());
        tokio::select! {
            _ = sleep(wait) => {}
            _ = stopped(&mut shutdown) => return Stop::Shutdown,
        }
    }
}

/// The token errors that end `run`.
fn permanent(e: &QuomeError) -> Option<Stop> {
    match e {
        QuomeError::CloudDisconnected | QuomeError::CloudInvalidGrant => Some(Stop::Revoked),
        QuomeError::ApiRefused { status, reason, .. }
            if *status == 403 && reason == "capability_missing" =>
        {
            Some(Stop::MissingCapability)
        }
        _ => None,
    }
}

/// The token response's URL, else the fallback: `wss://`, or `ws://` to this
/// computer only.
pub fn tunnel_url(token_url: Option<&str>, fallback: &str) -> Result<url::Url, QuomeError> {
    let raw = token_url
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback)
        .trim();
    let u = url::Url::parse(raw)
        .map_err(|e| QuomeError::Usage(format!("the tunnel URL {raw:?} is not valid ({e})")))?;
    match u.scheme() {
        "wss" if u.host().is_some() => Ok(u),
        "ws" if crate::url_policy::is_loopback(&u) => Ok(u),
        _ => Err(QuomeError::Usage(format!(
            "the tunnel URL {raw:?} must be wss:// (ws:// only to this computer)"
        ))),
    }
}

/// Jittered exponential backoff: attempt `n` waits between `min` and
/// `min(max, min·2ⁿ)`.
#[derive(Debug, Clone)]
pub(crate) struct Backoff {
    min: Duration,
    max: Duration,
    attempt: u32,
}

impl Backoff {
    pub(crate) fn new(min: Duration, max: Duration) -> Self {
        Backoff {
            min,
            max,
            attempt: 0,
        }
    }

    /// The longest the next wait may be.
    pub(crate) fn ceiling(&self) -> Duration {
        let factor = 1u32.checked_shl(self.attempt).unwrap_or(u32::MAX);
        self.min.saturating_mul(factor).min(self.max).max(self.min)
    }

    /// The next wait; `rnd` is a uniformly random number.
    pub(crate) fn next(&mut self, rnd: u64) -> Duration {
        let ceiling = self.ceiling();
        self.attempt = self.attempt.saturating_add(1);
        let span = u64::try_from((ceiling - self.min).as_nanos()).unwrap_or(u64::MAX);
        let jitter = if span == 0 {
            0
        } else {
            rnd % span.saturating_add(1)
        };
        self.min + Duration::from_nanos(jitter)
    }

    /// A tunnel that lived `healthy_after` starts the sequence over.
    pub(crate) fn tunnel_ended(&mut self, lived: Duration, healthy_after: Duration) {
        if lived >= healthy_after {
            self.attempt = 0;
        }
    }
}

/// Resolves once a stop was asked for (never, when nobody can ask).
async fn stopped(rx: &mut watch::Receiver<bool>) {
    if rx.wait_for(|stop| *stop).await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// The program path and what it resolved to at start.
struct Program {
    path: PathBuf,
    at_start: Option<PathBuf>,
}

impl Program {
    fn new(path: PathBuf) -> Self {
        let at_start = std::fs::canonicalize(&path).ok();
        Program { path, at_start }
    }

    /// True when the path now resolves elsewhere. A path that cannot be
    /// resolved right now (mid-upgrade) is not a change yet.
    fn changed(&self) -> bool {
        match (&self.at_start, std::fs::canonicalize(&self.path)) {
            (Some(was), Ok(now)) => *was != now,
            _ => false,
        }
    }
}

// ------------------------------------------------------------- one tunnel

enum End {
    /// The tunnel (or its dial) ended; `code` is the gateway's close code, if any.
    Closed {
        code: Option<u16>,
        reason: &'static str,
    },
    /// No tunnel was attempted (no token, or no usable URL).
    Failed,
    Shutdown,
    Upgraded,
}

struct Outcome {
    end: End,
    /// How long the tunnel was up.
    lived: Duration,
}

impl Outcome {
    fn failed() -> Self {
        Outcome {
            end: End::Failed,
            lived: Duration::ZERO,
        }
    }

    fn closed(code: Option<u16>, reason: &'static str) -> Self {
        Outcome {
            end: End::Closed { code, reason },
            lived: Duration::ZERO,
        }
    }
}

/// Aborts the task when dropped.
struct Task<T>(JoinHandle<T>);

impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Messages from the session's background work.
enum Ev {
    Advert(Advert),
    Token(Result<ModelsToken, QuomeError>),
}

fn lifetime(t: &ModelsToken) -> Duration {
    Duration::from_secs(if t.expires_in == 0 {
        DEFAULT_TOKEN_SECS
    } else {
        t.expires_in
    })
}

/// When to refresh a token of this lifetime: `refresh_before` ahead of its
/// end, but never sooner than `min(10 s, lifetime / 2)` after it was fetched.
fn refresh_after(lifetime: Duration, refresh_before: Duration) -> Duration {
    lifetime
        .saturating_sub(refresh_before)
        .max(REFRESH_RETRY.min(lifetime / 2))
}

async fn send_line(w: &mut WriteHalf<TStream>, m: &ControlMsg) -> std::io::Result<()> {
    w.write_all(encode_line(m).as_bytes()).await?;
    w.flush().await
}

enum Unsent {
    /// The stream is gone: the tunnel is ending, the pump knows why.
    Closed,
    /// The gateway stopped reading the control stream.
    Stalled,
}

/// A control line, bounded by `within`.
async fn send_control(
    w: &mut WriteHalf<TStream>,
    m: &ControlMsg,
    within: Duration,
) -> Result<(), Unsent> {
    match tokio::time::timeout(within, send_line(w, m)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) => Err(Unsent::Closed),
        Err(_) => Err(Unsent::Stalled),
    }
}

async fn unsent(why: Unsent, pump: &mut Task<PumpEnd>) -> Outcome {
    match why {
        Unsent::Closed => wait_pump(pump, "control_stream_closed").await,
        Unsent::Stalled => Outcome::closed(None, "control_stream_stalled"),
    }
}

fn pump_end(end: PumpEnd) -> Outcome {
    match end {
        PumpEnd::PeerClosed(code) => Outcome::closed(code, "closed"),
        PumpEnd::Dead => Outcome::closed(None, "no_pong"),
        PumpEnd::TextFrame => Outcome::closed(None, "text_frame"),
        PumpEnd::LocalEof => Outcome::closed(None, "closed"),
        PumpEnd::Error => Outcome::closed(None, "websocket_error"),
    }
}

/// Dial, run the tunnel until it ends, and say why.
async fn connection(
    deps: &mut TunnelDeps,
    proxy: &Arc<ProxyState>,
    program: Option<&Program>,
    token: ModelsToken,
    url: url::Url,
) -> Outcome {
    let fetched_at = Instant::now();
    let mut shutdown = deps.shutdown.clone();
    let t = deps.timing.clone();
    let first = tokio::select! {
        a = (deps.advert)() => a,
        _ = stopped(&mut shutdown) => return Outcome { end: End::Shutdown, lived: Duration::ZERO },
    };
    let dialed = tokio::select! {
        r = tokio::time::timeout(DIAL_TIMEOUT, dial(&url, &token.access_token)) => r,
        _ = stopped(&mut shutdown) => return Outcome { end: End::Shutdown, lived: Duration::ZERO },
    };
    let ws = match dialed {
        Ok(Ok(ws)) => ws,
        Ok(Err(DialError::Status(401))) => {
            return Outcome::closed(Some(CLOSE_BAD_TOKEN), "refused")
        }
        Ok(Err(DialError::Status(403))) => {
            return Outcome::closed(Some(CLOSE_NOT_ACTIVE), "refused")
        }
        Ok(Err(DialError::Status(s))) => {
            log::event("dial_failed", &[("status", s.to_string())]);
            return Outcome::closed(None, "refused");
        }
        Ok(Err(DialError::Failed(e))) => {
            log::event("dial_failed", &[("error", e)]);
            return Outcome::closed(None, "dial_failed");
        }
        Err(_) => {
            log::event("dial_failed", &[("error", "timed out".to_string())]);
            return Outcome::closed(None, "dial_failed");
        }
    };
    let up = Instant::now();

    let (ws_end, mux_end) = tokio::io::duplex(PIPE_BYTES);
    let mut pump_task = Task(tokio::spawn(pump(ws, ws_end, t.ping_every, t.dead_after)));
    let (in_tx, mut inbound) = mpsc::unbounded_channel::<TStream>();
    let (mux, driver) = spawn_mux(mux_end, yamux::Mode::Client, move |s| {
        let _ = in_tx.send(s);
    });
    let mut driver = Task(driver);

    let control = match mux.open().await {
        Ok(s) => s,
        Err(_) => return wait_pump(&mut pump_task, "yamux_error").await,
    };
    let (rd, mut wr) = tokio::io::split(control);
    let mut lines = BufReader::new(rd).lines();
    let hello = ControlMsg::Hello {
        agent_version: env!("CARGO_PKG_VERSION").to_string(),
        advert: first,
    };
    if let Err(why) = send_control(&mut wr, &hello, t.dead_after).await {
        return unsent(why, &mut pump_task).await;
    }

    let mut tasks: JoinSet<()> = JoinSet::new();
    let mut background: JoinSet<()> = JoinSet::new();
    let (ev_tx, mut events) = mpsc::unbounded_channel::<Ev>();
    let mut adverts = tokio::time::interval_at(up + t.advert_every, t.advert_every);
    let mut upgrades = tokio::time::interval_at(up + t.upgrade_every, t.upgrade_every);
    let life = lifetime(&token);
    let mut lapse_at = fetched_at + life;
    let mut refresh_at = fetched_at + refresh_after(life, t.refresh_before);
    let mut advert_busy = false;
    let mut refresh_busy = false;

    let end = loop {
        tokio::select! {
            Some(s) = inbound.recv() => {
                tasks.spawn(serve_stream(s, proxy.clone()));
            }
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
            line = lines.next_line() => match line {
                Ok(Some(l)) => match decode_line(&l) {
                    Ok(ControlMsg::Welcome { gateway_key, machine_id }) => {
                        match GatewayKey::from_b64url(&gateway_key) {
                            Ok(k) => {
                                *proxy.verifier.lock().await =
                                    Some(RequestTokenVerifier::new(k, deps.audience.clone()));
                                log::event("connected", &[("machine_id", machine_id)]);
                            }
                            Err(_) => break Outcome::closed(None, "bad_gateway_key"),
                        }
                    }
                    Ok(_) => {}
                    Err(_) => log::event("control_line_ignored", &[]),
                },
                // The control stream ended: the tunnel is going; the pump knows why.
                _ => break wait_pump(&mut pump_task, "control_stream_closed").await,
            },
            _ = adverts.tick(), if !advert_busy => {
                advert_busy = true;
                let f = (deps.advert)();
                let tx = ev_tx.clone();
                background.spawn(async move {
                    let _ = tx.send(Ev::Advert(f.await));
                });
            }
            _ = sleep_until(refresh_at), if !refresh_busy => {
                refresh_busy = true;
                let f = (deps.token)();
                let tx = ev_tx.clone();
                background.spawn(async move {
                    let _ = tx.send(Ev::Token(f.await));
                });
            }
            Some(ev) = events.recv() => match ev {
                Ev::Advert(advert) => {
                    advert_busy = false;
                    let msg = ControlMsg::Advert { advert };
                    if let Err(why) = send_control(&mut wr, &msg, t.dead_after).await {
                        break unsent(why, &mut pump_task).await;
                    }
                }
                Ev::Token(Ok(fresh)) => {
                    refresh_busy = false;
                    let now = Instant::now();
                    let life = lifetime(&fresh);
                    let msg = ControlMsg::Token { access_token: fresh.access_token };
                    if let Err(why) = send_control(&mut wr, &msg, t.dead_after).await {
                        break unsent(why, &mut pump_task).await;
                    }
                    lapse_at = now + life;
                    refresh_at = now + refresh_after(life, t.refresh_before);
                    log::event("token_refreshed", &[]);
                }
                Ev::Token(Err(e)) => {
                    refresh_busy = false;
                    refresh_at = Instant::now() + REFRESH_RETRY;
                    log::event("token_refresh_failed", &[("error", e.to_string())]);
                }
            },
            _ = sleep_until(lapse_at) => break Outcome::closed(None, "token_lapsed"),
            r = &mut pump_task.0 => {
                break r.map(pump_end).unwrap_or_else(|_| Outcome::closed(None, "websocket_error"));
            }
            _ = &mut driver.0 => break wait_pump(&mut pump_task, "yamux_closed").await,
            _ = stopped(&mut shutdown) => break Outcome { end: End::Shutdown, lived: Duration::ZERO },
            _ = upgrades.tick() => {
                if program.is_some_and(Program::changed) {
                    break Outcome { end: End::Upgraded, lived: Duration::ZERO };
                }
            }
        }
    };
    let lived = up.elapsed();
    if matches!(end.end, End::Shutdown | End::Upgraded) {
        // Stop taking streams; let the ones in flight finish, within `drain`.
        let deadline = Instant::now() + t.drain;
        loop {
            tokio::select! {
                r = tasks.join_next() => if r.is_none() { break },
                Some(s) = inbound.recv() => drop(s),
                _ = sleep_until(deadline) => break,
            }
        }
        // Closing the last handle closes yamux, then the WebSocket.
        drop((tasks, lines, wr, mux, inbound));
        let _ = tokio::time::timeout(CLOSE_GRACE, &mut pump_task.0).await;
    }
    Outcome {
        end: end.end,
        lived,
    }
}

/// The pump's reason, if it gives one within a moment.
async fn wait_pump(pump: &mut Task<PumpEnd>, otherwise: &'static str) -> Outcome {
    match tokio::time::timeout(CLOSE_GRACE, &mut pump.0).await {
        Ok(Ok(end)) => pump_end(end),
        _ => Outcome::closed(None, otherwise),
    }
}

// ----------------------------------------------------------------- dial

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

type Ws = WebSocketStream<Box<dyn Io>>;

enum DialError {
    /// The gateway answered the upgrade with this HTTP status.
    Status(u16),
    Failed(String),
}

/// TCP, TLS for `wss://` (the bundled web roots), then the WebSocket upgrade
/// with the bearer token.
async fn dial(u: &url::Url, bearer: &str) -> Result<Ws, DialError> {
    let host = match u.host() {
        Some(url::Host::Domain(d)) => d.to_string(),
        Some(url::Host::Ipv4(ip)) => ip.to_string(),
        Some(url::Host::Ipv6(ip)) => ip.to_string(),
        None => return Err(DialError::Failed("the tunnel URL has no host".into())),
    };
    let port = u.port_or_known_default().unwrap_or(443);
    let tcp = TcpStream::connect((host.as_str(), port))
        .await
        .map_err(|e| DialError::Failed(e.to_string()))?;
    let _ = tcp.set_nodelay(true);
    let io: Box<dyn Io> = if u.scheme() == "wss" {
        Box::new(tls(host, tcp).await?)
    } else {
        Box::new(tcp)
    };
    let mut req = u
        .as_str()
        .into_client_request()
        .map_err(|e| DialError::Failed(e.to_string()))?;
    let mut v = HeaderValue::from_str(&format!("Bearer {bearer}"))
        .map_err(|_| DialError::Failed("the token is not a valid header value".into()))?;
    v.set_sensitive(true);
    req.headers_mut().insert(AUTHORIZATION, v);
    match tokio_tungstenite::client_async(req, io).await {
        Ok((ws, _)) => Ok(ws),
        Err(tungstenite::Error::Http(resp)) => Err(DialError::Status(resp.status().as_u16())),
        Err(e) => Err(DialError::Failed(e.to_string())),
    }
}

async fn tls(
    host: String,
    tcp: TcpStream,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, DialError> {
    use tokio_rustls::rustls;
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| DialError::Failed(e.to_string()))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from(host)
        .map_err(|e| DialError::Failed(e.to_string()))?;
    tokio_rustls::TlsConnector::from(Arc::new(cfg))
        .connect(name, tcp)
        .await
        .map_err(|e| DialError::Failed(e.to_string()))
}

// ---------------------------------------------------------- yamux driver

/// A yamux stream as tokio and hyper see it.
type TStream = Compat<yamux::Stream>;
type OpenReply = oneshot::Sender<Result<TStream, yamux::ConnectionError>>;

/// A handle on the driver task; dropping every clone closes the connection.
#[derive(Clone, Debug)]
struct Mux {
    open: mpsc::Sender<OpenReply>,
}

impl Mux {
    async fn open(&self) -> Result<TStream, yamux::ConnectionError> {
        let (tx, rx) = oneshot::channel();
        self.open
            .send(tx)
            .await
            .map_err(|_| yamux::ConnectionError::Closed)?;
        rx.await.map_err(|_| yamux::ConnectionError::Closed)?
    }
}

/// The one task that owns the yamux `Connection`. It keeps polling
/// `poll_next_inbound`, which moves every frame (outbound ones too), hands
/// each peer-opened stream to `on_inbound`, and ends when the connection does
/// (peer EOF, an error, or every `Mux` dropped, which sends GoAway first).
fn spawn_mux<T, F>(
    io: T,
    mode: yamux::Mode,
    mut on_inbound: F,
) -> (Mux, JoinHandle<Result<(), yamux::ConnectionError>>)
where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    F: FnMut(TStream) + Send + 'static,
{
    let mut cfg = yamux::Config::default();
    cfg.set_max_num_streams(YAMUX_MAX_STREAMS);
    let mut conn = yamux::Connection::new(io.compat(), cfg, mode);
    let (open_tx, mut open_rx) = mpsc::channel::<OpenReply>(16);
    let task = tokio::spawn(async move {
        let mut pending: Option<OpenReply> = None;
        let mut handles_gone = false;
        let res = poll_fn(|cx| loop {
            if pending.is_none() && !handles_gone {
                match open_rx.poll_recv(cx) {
                    Poll::Ready(Some(tx)) => pending = Some(tx),
                    Poll::Ready(None) => handles_gone = true,
                    Poll::Pending => {}
                }
            }
            if let Some(tx) = pending.take() {
                match conn.poll_new_outbound(cx) {
                    Poll::Ready(r) => {
                        let _ = tx.send(r.map(|s| s.compat()));
                        continue;
                    }
                    Poll::Pending => pending = Some(tx),
                }
            }
            if handles_gone {
                return Poll::Ready(Ok(()));
            }
            match conn.poll_next_inbound(cx) {
                Poll::Ready(Some(Ok(s))) => on_inbound(s.compat()),
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Err(e)),
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Pending => return Poll::Pending,
            }
        })
        .await;
        let _ = poll_fn(|cx| conn.poll_close(cx)).await;
        res
    });
    (Mux { open: open_tx }, task)
}

// --------------------------------------------------------- WS ⇄ duplex

#[derive(Debug, PartialEq)]
enum PumpEnd {
    PeerClosed(Option<u16>),
    /// No Pong within `dead_after` (or a send stuck that long).
    Dead,
    /// A text message: refused with 1003.
    TextFrame,
    /// The yamux side hung up.
    LocalEof,
    Error,
}

/// Binary WebSocket messages ⇄ one end of a duplex. Sends a Ping every
/// `ping_every` and ends `Dead` exactly `dead_after` after the last Pong
/// (or the start), not at the next ping tick.
/// Incoming Pings are answered by tungstenite itself.
async fn pump<S>(
    ws: WebSocketStream<S>,
    pipe: DuplexStream,
    ping_every: Duration,
    dead_after: Duration,
) -> PumpEnd
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut sink, mut stream) = ws.split();
    let (mut rd, mut wr) = tokio::io::split(pipe);
    let mut buf = vec![0u8; 64 * 1024];
    let mut tick = tokio::time::interval_at(Instant::now() + ping_every, ping_every);
    let mut last_pong = Instant::now();
    macro_rules! send {
        ($m:expr) => {
            match tokio::time::timeout(dead_after, sink.send($m)).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => return PumpEnd::Error,
                Err(_) => return PumpEnd::Dead,
            }
        };
    }
    loop {
        tokio::select! {
            msg = stream.next() => match msg {
                Some(Ok(Message::Binary(b))) => {
                    if wr.write_all(&b).await.is_err() {
                        return PumpEnd::LocalEof;
                    }
                }
                Some(Ok(Message::Pong(_))) => last_pong = Instant::now(),
                Some(Ok(Message::Ping(_))) | Some(Ok(Message::Frame(_))) => {}
                Some(Ok(Message::Text(_))) => {
                    let close = CloseFrame { code: CloseCode::Unsupported, reason: "binary only".into() };
                    let _ = tokio::time::timeout(CLOSE_GRACE, sink.send(Message::Close(Some(close)))).await;
                    return PumpEnd::TextFrame;
                }
                Some(Ok(Message::Close(f))) => return PumpEnd::PeerClosed(f.map(|f| f.code.into())),
                Some(Err(_)) => return PumpEnd::Error,
                None => return PumpEnd::PeerClosed(None),
            },
            n = rd.read(&mut buf) => match n {
                Ok(0) | Err(_) => {
                    let _ = tokio::time::timeout(CLOSE_GRACE, sink.close()).await;
                    return PumpEnd::LocalEof;
                }
                Ok(n) => send!(Message::Binary(buf[..n].to_vec().into())),
            },
            _ = sleep_until(last_pong + dead_after) => return PumpEnd::Dead,
            _ = tick.tick() => send!(Message::Ping(Vec::new().into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::protocol::AdvertModel;
    use crate::machine::proxy::local_client;
    use crate::machine::proxy::tests::{fixed_now, good, vec_str, CHAT};
    use http_body_util::{BodyExt, Full};
    use hyper_util::rt::TokioIo;
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;
    use tokio::io::{Lines, ReadHalf};
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const BOUND: Duration = Duration::from_secs(10);

    async fn bounded<F: std::future::Future>(what: &str, f: F) -> F::Output {
        tokio::time::timeout(BOUND, f)
            .await
            .unwrap_or_else(|_| panic!("timed out: {what}"))
    }

    // ------------------------------------------------------ fake gateway

    #[derive(Clone, Copy)]
    enum Mode {
        Normal,
        /// Accept the WebSocket, then never read it again (no Pongs).
        Silent,
        /// Close at once with this code.
        Close(u16),
    }

    struct GwConn {
        bearer: Option<String>,
        at: Instant,
        mux: Option<Mux>,
        inbound: Option<mpsc::UnboundedReceiver<TStream>>,
    }

    type CtlLines = Lines<BufReader<ReadHalf<TStream>>>;

    impl GwConn {
        /// The machine's control stream (the first stream it opens).
        async fn control(&mut self) -> (CtlLines, WriteHalf<TStream>) {
            let s = bounded("the control stream", self.inbound.as_mut().unwrap().recv())
                .await
                .expect("the machine opens a control stream");
            let (r, w) = tokio::io::split(s);
            (BufReader::new(r).lines(), w)
        }

        fn mux(&self) -> Mux {
            self.mux.clone().unwrap()
        }
    }

    struct Gw {
        addr: SocketAddr,
        conns: mpsc::UnboundedReceiver<GwConn>,
    }

    impl Gw {
        async fn start(modes: Vec<Mode>) -> Gw {
            let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = l.local_addr().unwrap();
            let (tx, conns) = mpsc::unbounded_channel();
            tokio::spawn(async move {
                let mut n = 0;
                loop {
                    let Ok((tcp, _)) = l.accept().await else {
                        return;
                    };
                    let mode = modes.get(n).copied().unwrap_or(Mode::Normal);
                    n += 1;
                    let tx = tx.clone();
                    tokio::spawn(async move { accept(tcp, mode, tx).await });
                }
            });
            Gw { addr, conns }
        }

        fn url(&self) -> String {
            format!("ws://{}/acme/machine", self.addr)
        }

        async fn next(&mut self) -> GwConn {
            bounded("the machine dials", self.conns.recv())
                .await
                .expect("gateway running")
        }
    }

    async fn accept(tcp: TcpStream, mode: Mode, tx: mpsc::UnboundedSender<GwConn>) {
        let at = Instant::now();
        let seen = Arc::new(Mutex::new(None));
        let seen2 = seen.clone();
        // tungstenite's callback signature fixes the error type.
        #[allow(clippy::result_large_err)]
        let cb = move |req: &Request, resp: Response| {
            *seen2.lock().unwrap() = req
                .headers()
                .get("authorization")
                .map(|v| v.to_str().unwrap().to_string());
            Ok(resp)
        };
        let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(tcp, cb).await else {
            return;
        };
        let bearer = seen.lock().unwrap().clone();
        match mode {
            Mode::Normal => {
                let (a, b) = tokio::io::duplex(PIPE_BYTES);
                tokio::spawn(pump(
                    ws,
                    a,
                    Duration::from_secs(3600),
                    Duration::from_secs(7200),
                ));
                let (in_tx, inbound) = mpsc::unbounded_channel();
                let (mux, _driver) = spawn_mux(b, yamux::Mode::Server, move |s| {
                    let _ = in_tx.send(s);
                });
                let _ = tx.send(GwConn {
                    bearer,
                    at,
                    mux: Some(mux),
                    inbound: Some(inbound),
                });
            }
            Mode::Silent => {
                let _ = tx.send(GwConn {
                    bearer,
                    at,
                    mux: None,
                    inbound: None,
                });
                sleep(Duration::from_secs(3600)).await;
                drop(ws);
            }
            Mode::Close(code) => {
                let _ = tx.send(GwConn {
                    bearer,
                    at,
                    mux: None,
                    inbound: None,
                });
                let frame = CloseFrame {
                    code: CloseCode::from(code),
                    reason: "".into(),
                };
                let _ = ws.close(Some(frame)).await;
                while let Some(Ok(_)) = ws.next().await {}
            }
        }
    }

    /// One HTTP/1.1 request on a gateway-opened stream.
    async fn request(mux: &Mux, m: &str, p: &str, tok: Option<String>) -> (u16, Vec<u8>) {
        let s = mux.open().await.unwrap();
        let (mut tx, conn) = hyper::client::conn::http1::handshake(TokioIo::new(s))
            .await
            .unwrap();
        tokio::spawn(conn);
        let mut b = hyper::Request::builder()
            .method(m)
            .uri(p)
            .header("host", "machine");
        if let Some(t) = tok {
            b = b.header("x-quome-request-token", t);
        }
        let resp = tx
            .send_request(b.body(Full::new(Bytes::from_static(CHAT))).unwrap())
            .await
            .unwrap();
        let status = resp.status().as_u16();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        (status, body.to_vec())
    }

    use bytes::Bytes;

    async fn next_msg(lines: &mut CtlLines) -> (String, ControlMsg) {
        let l = lines.next_line().await.unwrap().expect("a control line");
        let m = decode_line(&l).unwrap();
        (l, m)
    }

    async fn welcome(w: &mut WriteHalf<TStream>) {
        let m = ControlMsg::Welcome {
            gateway_key: vec_str("gateway_key"),
            machine_id: "m-1".into(),
        };
        send_line(w, &m).await.unwrap();
    }

    async fn until_verifier(p: &ProxyState) {
        bounded("the welcome is installed", async {
            while p.verifier.lock().await.is_none() {
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
    }

    // ------------------------------------------------------ fake machine

    fn fake_advert() -> Advert {
        Advert {
            server: "ollama".into(),
            models: vec![AdvertModel {
                id: "qwen3:32b".into(),
                context_length: None,
            }],
            memory_total_bytes: 64 << 30,
            memory_free_bytes: 32 << 30,
            accelerator: "apple-m3-ultra".into(),
            max_concurrency: 4,
            healthy: true,
            pool: None,
        }
    }

    fn quiet() -> Timing {
        Timing {
            ping_every: Duration::from_secs(3600),
            dead_after: Duration::from_secs(7200),
            advert_every: Duration::from_secs(3600),
            refresh_before: Duration::from_secs(60),
            backoff_min: Duration::from_millis(10),
            backoff_max: Duration::from_millis(50),
            healthy_after: Duration::from_secs(60),
            upgrade_every: Duration::from_secs(3600),
            drain: Duration::from_secs(10),
        }
    }

    struct Fake {
        deps: TunnelDeps,
        stop: watch::Sender<bool>,
        token_calls: Arc<AtomicUsize>,
        advert_calls: Arc<AtomicUsize>,
    }

    /// Tokens `t1`, `t2`, … of `expires_in` seconds, or the error `fail` makes.
    fn fake(url: &str, expires_in: u64, timing: Timing, fail: Option<fn() -> QuomeError>) -> Fake {
        let token_calls = Arc::new(AtomicUsize::new(0));
        let advert_calls = Arc::new(AtomicUsize::new(0));
        let tc = token_calls.clone();
        let ac = advert_calls.clone();
        let token: TokenFn = Arc::new(move || {
            let n = tc.fetch_add(1, Ordering::SeqCst) + 1;
            Box::pin(async move {
                match fail {
                    Some(f) => Err(f()),
                    None => Ok(ModelsToken {
                        access_token: format!("t{n}"),
                        expires_in,
                        url: None,
                    }),
                }
            })
        });
        let advert: AdvertFn = Arc::new(move || {
            ac.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { fake_advert() })
        });
        let (stop, shutdown) = watch::channel(false);
        Fake {
            deps: TunnelDeps {
                token,
                advert,
                audience: vec_str("audience"),
                fallback_url: url.into(),
                timing,
                program: None,
                shutdown,
            },
            stop,
            token_calls,
            advert_calls,
        }
    }

    fn proxy(local: &str) -> Arc<ProxyState> {
        Arc::new(ProxyState {
            verifier: tokio::sync::Mutex::new(None),
            local: url::Url::parse(local).unwrap(),
            local_auth: None,
            client: local_client(),
            slots: tokio::sync::Semaphore::new(4),
            responses_supported: AtomicBool::new(true),
            clock: fixed_now,
        })
    }

    async fn chat_local() -> MockServer {
        let local = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"id":"c-1","ok":true}"#))
            .mount(&local)
            .await;
        local
    }

    // ------------------------------------------------------------- tests

    #[tokio::test]
    async fn hello_then_welcome_then_a_request() {
        let local = chat_local().await;
        let mut gw = Gw::start(vec![]).await;
        let f = fake(&gw.url(), 300, quiet(), None);
        let p = proxy(&local.uri());
        let running = tokio::spawn(run(f.deps, p.clone()));

        let mut c = gw.next().await;
        assert_eq!(c.bearer.as_deref(), Some("Bearer t1"));
        let (mut lines, mut w) = c.control().await;
        let (_, hello) = bounded("hello", next_msg(&mut lines)).await;
        assert_eq!(
            hello,
            ControlMsg::Hello {
                agent_version: env!("CARGO_PKG_VERSION").into(),
                advert: fake_advert(),
            }
        );
        welcome(&mut w).await;
        until_verifier(&p).await;

        let (status, body) = bounded(
            "the request",
            request(&c.mux(), "POST", "/v1/chat/completions", Some(good("r-1"))),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(body, br#"{"id":"c-1","ok":true}"#);
        let seen = local.received_requests().await.unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].body, CHAT);

        f.stop.send(true).unwrap();
        assert_eq!(bounded("stop", running).await.unwrap(), Stop::Shutdown);
        assert_eq!(f.token_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn refreshes_the_token_on_the_control_stream() {
        let mut gw = Gw::start(vec![]).await;
        let t0 = Instant::now();
        let f = fake(&gw.url(), 70, quiet(), None);
        let _running = Task(tokio::spawn(run(f.deps, proxy("http://127.0.0.1:9"))));
        let mut c = gw.next().await;
        let (mut lines, _w) = c.control().await;
        bounded("hello", next_msg(&mut lines)).await;

        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(11)).await;
        let (raw, m) = tokio::time::timeout(Duration::from_secs(30), next_msg(&mut lines))
            .await
            .expect("a token line before the old token lapses");
        assert_eq!(raw, r#"{"type":"token","access_token":"t2"}"#);
        assert_eq!(
            m,
            ControlMsg::Token {
                access_token: "t2".into()
            }
        );
        assert!(t0.elapsed() >= Duration::from_secs(10));
        assert!(f.token_calls.load(Ordering::SeqCst) >= 2);
    }

    #[tokio::test]
    async fn adverts_every_interval() {
        let mut gw = Gw::start(vec![]).await;
        let t0 = Instant::now();
        let mut timing = quiet();
        timing.advert_every = Duration::from_secs(30);
        let f = fake(&gw.url(), 300, timing, None);
        let p = proxy("http://127.0.0.1:9");
        let _running = Task(tokio::spawn(run(f.deps, p)));
        let mut c = gw.next().await;
        let (mut lines, _w) = c.control().await;
        bounded("hello", next_msg(&mut lines)).await;

        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(30)).await;
        let (_, m) = tokio::time::timeout(Duration::from_secs(60), next_msg(&mut lines))
            .await
            .expect("a second advert");
        assert_eq!(
            m,
            ControlMsg::Advert {
                advert: fake_advert()
            }
        );
        assert!(t0.elapsed() >= Duration::from_secs(30));
        // The paused clock may run on to the next tick while the line is in flight.
        assert!(f.advert_calls.load(Ordering::SeqCst) >= 2);
    }

    #[tokio::test]
    async fn reconnects_with_a_fresh_token_after_silence() {
        let mut gw = Gw::start(vec![Mode::Silent]).await;
        let mut timing = quiet();
        timing.ping_every = Duration::from_millis(50);
        timing.dead_after = Duration::from_millis(200);
        let f = fake(&gw.url(), 300, timing, None);
        let _running = Task(tokio::spawn(run(f.deps, proxy("http://127.0.0.1:9"))));

        let first = gw.next().await;
        assert_eq!(first.bearer.as_deref(), Some("Bearer t1"));
        let mut second = gw.next().await;
        assert_eq!(second.bearer.as_deref(), Some("Bearer t2"));
        let gap = second.at.duration_since(first.at);
        assert!(gap >= Duration::from_millis(200), "{gap:?}");
        // dead_after, at most one backoff (50 ms) and a redial.
        assert!(gap < Duration::from_millis(600), "{gap:?}");
        assert_eq!(f.token_calls.load(Ordering::SeqCst), 2);
        let (mut lines, _w) = second.control().await;
        assert!(matches!(
            bounded("hello", next_msg(&mut lines)).await.1,
            ControlMsg::Hello { .. }
        ));
    }

    /// The peer answers three Pings, then stops reading: the pump must call
    /// the tunnel dead `dead_after` after that last Pong, not a ping tick later.
    #[tokio::test]
    async fn a_silent_peer_is_dead_within_the_window_of_its_last_pong() {
        let ping_every = Duration::from_millis(400);
        let dead_after = Duration::from_millis(600);
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let (last_tx, last_rx) = oneshot::channel::<Instant>();
        tokio::spawn(async move {
            let (tcp, _) = l.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            let mut pings = 0;
            while let Some(Ok(m)) = ws.next().await {
                if let Message::Ping(_) = m {
                    pings += 1;
                    if pings == 3 {
                        ws.flush().await.unwrap(); // sends the queued Pong
                        let _ = last_tx.send(Instant::now());
                        break;
                    }
                }
            }
            sleep(Duration::from_secs(30)).await; // never reads again
            drop(ws);
        });
        let tcp = TcpStream::connect(addr).await.unwrap();
        let (ws, _) = tokio_tungstenite::client_async(format!("ws://{addr}/m"), tcp)
            .await
            .unwrap();
        let (a, _b) = tokio::io::duplex(1024);
        let end = bounded("dead", pump(ws, a, ping_every, dead_after)).await;
        let dead_at = Instant::now();
        assert_eq!(end, PumpEnd::Dead);
        let last = last_rx.await.unwrap();
        let took = dead_at.duration_since(last);
        assert!(took >= dead_after - Duration::from_millis(50), "{took:?}");
        assert!(took < dead_after + Duration::from_millis(150), "{took:?}");
    }

    /// A gateway that answers Pings but stops reading the control stream:
    /// a stuck control write ends the tunnel within `dead_after`.
    #[tokio::test]
    async fn a_stalled_control_stream_ends_the_tunnel() {
        let mut gw = Gw::start(vec![]).await;
        let mut timing = quiet();
        timing.advert_every = Duration::from_millis(10);
        // Pings keep being answered, so only the stalled write can end it.
        timing.ping_every = Duration::from_millis(50);
        timing.dead_after = Duration::from_millis(300);
        let mut f = fake(&gw.url(), 300, timing, None);
        // ~100 KiB per advert: the 256 KiB stream window fills in a few.
        let mut big = fake_advert();
        big.models = (0..4000)
            .map(|i| AdvertModel {
                id: format!("model-{i:08}"),
                context_length: Some(8192),
            })
            .collect();
        f.deps.advert = Arc::new(move || {
            let a = big.clone();
            Box::pin(async move { a })
        });
        let _running = Task(tokio::spawn(run(f.deps, proxy("http://127.0.0.1:9"))));
        let first = gw.next().await; // keep it open, never read its streams
        let second = gw.next().await;
        assert_eq!(second.bearer.as_deref(), Some("Bearer t2"));
        drop(first);
    }

    async fn stops_without_dialing(err: fn() -> QuomeError, want: Stop) {
        let mut gw = Gw::start(vec![]).await;
        let f = fake(&gw.url(), 300, quiet(), Some(err));
        let got = bounded("run ends", run(f.deps, proxy("http://127.0.0.1:9"))).await;
        assert_eq!(got, want);
        assert!(gw.conns.try_recv().is_err(), "no dial");
        assert_eq!(f.token_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn revoked_stops() {
        stops_without_dialing(|| QuomeError::CloudDisconnected, Stop::Revoked).await;
        stops_without_dialing(|| QuomeError::CloudInvalidGrant, Stop::Revoked).await;
    }

    #[tokio::test]
    async fn capability_missing_stops() {
        stops_without_dialing(
            || QuomeError::ApiRefused {
                message: "missing".into(),
                status: 403,
                reason: "capability_missing".into(),
            },
            Stop::MissingCapability,
        )
        .await;
    }

    #[tokio::test]
    async fn other_token_errors_back_off_and_retry() {
        let gw = Gw::start(vec![]).await;
        let f = fake(
            &gw.url(),
            300,
            quiet(),
            Some(|| QuomeError::ApiRefused {
                message: "boom".into(),
                status: 500,
                reason: "internal".into(),
            }),
        );
        let running = tokio::spawn(run(f.deps, proxy("http://127.0.0.1:9")));
        bounded("retries", async {
            while f.token_calls.load(Ordering::SeqCst) < 3 {
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        assert!(!running.is_finished(), "a transient failure never ends run");
        running.abort();
    }

    #[tokio::test]
    async fn close_4403_backs_off() {
        let mut gw = Gw::start(vec![Mode::Close(4403), Mode::Close(4403)]).await;
        let mut timing = quiet();
        timing.backoff_min = Duration::from_millis(100);
        timing.backoff_max = Duration::from_millis(200);
        let f = fake(&gw.url(), 300, timing, None);
        let running = tokio::spawn(run(f.deps, proxy("http://127.0.0.1:9")));
        let a = gw.next().await;
        let b = gw.next().await;
        let mut c = gw.next().await;
        assert!(b.at.duration_since(a.at) >= Duration::from_millis(100));
        assert!(c.at.duration_since(b.at) >= Duration::from_millis(100));
        let (mut lines, _w) = c.control().await;
        bounded("hello", next_msg(&mut lines)).await;
        assert!(!running.is_finished());
        running.abort();
    }

    #[tokio::test]
    async fn close_4401_refetches_the_token_at_once() {
        let mut gw = Gw::start(vec![Mode::Close(4401)]).await;
        let mut timing = quiet();
        timing.backoff_min = Duration::from_secs(3600);
        timing.backoff_max = Duration::from_secs(3600);
        let f = fake(&gw.url(), 300, timing, None);
        let _running = Task(tokio::spawn(run(f.deps, proxy("http://127.0.0.1:9"))));
        assert_eq!(gw.next().await.bearer.as_deref(), Some("Bearer t1"));
        assert_eq!(gw.next().await.bearer.as_deref(), Some("Bearer t2"));
    }

    #[tokio::test]
    async fn streams_before_welcome_are_refused() {
        let local = chat_local().await;
        let mut gw = Gw::start(vec![]).await;
        let f = fake(&gw.url(), 300, quiet(), None);
        let _running = Task(tokio::spawn(run(f.deps, proxy(&local.uri()))));
        let mut c = gw.next().await;
        let (mut lines, _w) = c.control().await;
        bounded("hello", next_msg(&mut lines)).await;
        let (status, body) = bounded(
            "the request",
            request(&c.mux(), "POST", "/v1/chat/completions", Some(good("r-1"))),
        )
        .await;
        assert_eq!(status, 502);
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"error":{"code":"request_token_refused","reason":"no_gateway_key"}})
        );
        assert!(local.received_requests().await.unwrap().is_empty());
    }

    #[test]
    fn backoff_bounds() {
        let min = Duration::from_secs(1);
        let max = Duration::from_secs(60);
        for rnd in [0, 1, u64::MAX / 3, u64::MAX] {
            let mut b = Backoff::new(min, max);
            for n in 0..40u32 {
                let cap = min.saturating_mul(2u32.saturating_pow(n)).min(max);
                assert_eq!(b.ceiling(), cap, "n={n}");
                let d = b.next(rnd);
                assert!(d >= min && d <= cap, "n={n} rnd={rnd} d={d:?}");
            }
            assert_eq!(b.ceiling(), max);
            b.tunnel_ended(Duration::from_secs(59), Duration::from_secs(60));
            assert_eq!(b.ceiling(), max, "a short tunnel keeps the backoff");
            b.tunnel_ended(Duration::from_secs(60), Duration::from_secs(60));
            assert_eq!(b.ceiling(), min, "a healthy tunnel resets it");
            assert_eq!(b.next(rnd), min);
        }
        let mut b = Backoff::new(min, max);
        b.next(0);
        assert_eq!(
            b.next(1_000_000_000),
            Duration::from_secs(2),
            "the top of the range"
        );
    }

    #[test]
    fn tunnel_url_policy() {
        let fb = "wss://models.apps.example/acme/machine";
        assert_eq!(
            tunnel_url(Some("wss://gw.example/acme/machine"), fb)
                .unwrap()
                .as_str(),
            "wss://gw.example/acme/machine"
        );
        assert_eq!(
            tunnel_url(Some("ws://127.0.0.1:8080/acme/machine"), fb)
                .unwrap()
                .as_str(),
            "ws://127.0.0.1:8080/acme/machine"
        );
        assert!(tunnel_url(Some("ws://localhost:8080/m"), fb).is_ok());
        assert!(tunnel_url(Some("ws://example.com/acme/machine"), fb).is_err());
        assert!(tunnel_url(Some("https://gw.example/acme/machine"), fb).is_err());
        assert!(tunnel_url(Some("not a url"), fb).is_err());
        assert_eq!(tunnel_url(None, fb).unwrap().as_str(), fb);
        assert_eq!(tunnel_url(Some(""), fb).unwrap().as_str(), fb);
        assert!(tunnel_url(None, "ws://example.com/m").is_err());
    }

    #[test]
    fn refresh_timing() {
        let s = Duration::from_secs;
        assert_eq!(refresh_after(s(70), s(60)), s(10));
        assert_eq!(refresh_after(s(300), s(60)), s(240));
        assert_eq!(refresh_after(s(30), s(60)), s(10));
        assert_eq!(refresh_after(s(4), s(60)), s(2));
    }

    // ---------------------------------- hang-ups, idle streams, stopping

    /// A raw local server: reports when a request head arrived and when the
    /// connection closed; answers after `answer_after` (never when `None`).
    async fn raw_local(
        answer_after: Option<Duration>,
    ) -> (String, oneshot::Receiver<()>, oneshot::Receiver<Instant>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let (head_tx, head_rx) = oneshot::channel();
        let (closed_tx, closed_rx) = oneshot::channel();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut b = [0u8; 4096];
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = s.read(&mut b).await.unwrap();
                if n == 0 {
                    return;
                }
                buf.extend_from_slice(&b[..n]);
            }
            let _ = head_tx.send(());
            if let Some(d) = answer_after {
                sleep(d).await;
                let _ = s
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 4\r\n\r\ndone")
                    .await;
            }
            loop {
                match s.read(&mut b).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            let _ = closed_tx.send(Instant::now());
        });
        (url, head_rx, closed_rx)
    }

    #[tokio::test]
    async fn a_reset_before_the_answer_drops_the_local_request() {
        let (url, head, closed) = raw_local(None).await;
        let mut gw = Gw::start(vec![]).await;
        let f = fake(&gw.url(), 300, quiet(), None);
        let p = proxy(&url);
        let _running = Task(tokio::spawn(run(f.deps, p.clone())));
        let mut c = gw.next().await;
        let (mut lines, mut w) = c.control().await;
        bounded("hello", next_msg(&mut lines)).await;
        welcome(&mut w).await;
        until_verifier(&p).await;

        let mux = c.mux();
        let caller = tokio::spawn(async move {
            request(&mux, "POST", "/v1/chat/completions", Some(good("r-1"))).await
        });
        bounded("the local server sees the request", head)
            .await
            .unwrap();
        // The caller hangs up before any answer byte: its stream is reset.
        caller.abort();
        let hung_up = Instant::now();
        let at = tokio::time::timeout(Duration::from_secs(1), closed)
            .await
            .expect("the local connection closes within 1 s of the reset")
            .unwrap();
        assert!(at.duration_since(hung_up) < Duration::from_secs(1));
        bounded("the slot comes back", async {
            while p.slots.available_permits() != 4 {
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
    }

    #[tokio::test]
    async fn a_stop_waits_for_requests_in_flight() {
        let (url, head, _closed) = raw_local(Some(Duration::from_millis(300))).await;
        let mut gw = Gw::start(vec![]).await;
        let f = fake(&gw.url(), 300, quiet(), None);
        let p = proxy(&url);
        let running = tokio::spawn(run(f.deps, p.clone()));
        let mut c = gw.next().await;
        let (mut lines, mut w) = c.control().await;
        bounded("hello", next_msg(&mut lines)).await;
        welcome(&mut w).await;
        until_verifier(&p).await;

        let mux = c.mux();
        let caller = tokio::spawn(async move {
            request(&mux, "POST", "/v1/chat/completions", Some(good("r-1"))).await
        });
        bounded("the request is in flight", head).await.unwrap();
        f.stop.send(true).unwrap();
        sleep(Duration::from_millis(50)).await;
        assert!(!running.is_finished(), "the stop waits for the request");
        // A stream opened while stopping is not served.
        let late = tokio::time::timeout(Duration::from_secs(2), async {
            let s = c.mux().open().await.ok()?;
            let (mut tx, conn) = hyper::client::conn::http1::handshake(TokioIo::new(s))
                .await
                .ok()?;
            tokio::spawn(conn);
            let r = hyper::Request::get("/v1/models")
                .header("x-quome-request-token", good("r-2"))
                .body(Full::new(Bytes::new()))
                .unwrap();
            tx.send_request(r).await.ok()
        })
        .await;
        assert!(matches!(late, Ok(None)), "a late stream is refused");
        let (status, body) = bounded("the answer", caller).await.unwrap();
        assert_eq!((status, body.as_slice()), (200, &b"done"[..]));
        assert_eq!(bounded("stop", running).await.unwrap(), Stop::Shutdown);
    }

    #[tokio::test]
    async fn a_stop_waits_no_longer_than_the_drain() {
        let (url, head, _closed) = raw_local(None).await;
        let mut gw = Gw::start(vec![]).await;
        let mut timing = quiet();
        timing.drain = Duration::from_millis(300);
        let f = fake(&gw.url(), 300, timing, None);
        let p = proxy(&url);
        let running = tokio::spawn(run(f.deps, p.clone()));
        let mut c = gw.next().await;
        let (mut lines, mut w) = c.control().await;
        bounded("hello", next_msg(&mut lines)).await;
        welcome(&mut w).await;
        until_verifier(&p).await;
        let mux = c.mux();
        let _caller = Task(tokio::spawn(async move {
            request(&mux, "POST", "/v1/chat/completions", Some(good("r-1"))).await
        }));
        bounded("the request is in flight", head).await.unwrap();
        let t = Instant::now();
        f.stop.send(true).unwrap();
        assert_eq!(bounded("stop", running).await.unwrap(), Stop::Shutdown);
        let took = t.elapsed();
        assert!(took >= Duration::from_millis(300), "{took:?}");
        assert!(took < Duration::from_secs(3), "{took:?}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_upgraded_program_stops_the_tunnel() {
        let d = tempfile::tempdir().unwrap();
        let (old, new, link) = (
            d.path().join("v1"),
            d.path().join("v2"),
            d.path().join("quome"),
        );
        std::fs::write(&old, "1").unwrap();
        std::fs::write(&new, "2").unwrap();
        std::os::unix::fs::symlink(&old, &link).unwrap();
        let mut gw = Gw::start(vec![]).await;
        let mut timing = quiet();
        timing.upgrade_every = Duration::from_millis(50);
        let mut f = fake(&gw.url(), 300, timing, None);
        f.deps.program = Some(link.clone());
        let running = tokio::spawn(run(f.deps, proxy("http://127.0.0.1:9")));
        let mut c = gw.next().await;
        let (mut lines, _w) = c.control().await;
        bounded("hello", next_msg(&mut lines)).await;
        sleep(Duration::from_millis(150)).await;
        assert!(!running.is_finished(), "an unchanged program keeps serving");
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&new, &link).unwrap();
        assert_eq!(bounded("upgrade", running).await.unwrap(), Stop::Upgraded);
    }
}
