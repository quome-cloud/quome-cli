//! The pool server's side of the advert: ask `GET /health` what the pool is, check
//! every field, and fill the advert's `pool`, memory and concurrency from it.
//! A body that fails a check is dropped whole; only a fixed reason is logged,
//! never the body.
use std::time::Duration;

use serde_json::Value;
use url::Url;

use super::protocol::{Advert, FastMode, Pool, PoolMember, PoolSetup, SetupMember};

const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_MEMBERS: usize = 5;
const MAX_NAME: usize = 64;
const MAX_DETAIL: usize = 200;
const SETUP_STATES: [&str; 5] = ["pairing", "installing", "checking", "serving", "degraded"];
const EARLY_STATES: [&str; 3] = ["pairing", "installing", "checking"];
const MEMBER_STATUSES: [&str; 9] = [
    "waiting",
    "pairing",
    "paired",
    "copying",
    "installing",
    "checking",
    "ready",
    "unreachable",
    "failed",
];

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PoolProbe {
    pub pool: Option<Pool>,
    pub healthy: bool,
    pub memory_total_bytes: Option<u64>,
    pub memory_free_bytes: Option<u64>,
    pub max_concurrency: Option<u32>,
}

fn name_of(v: &Value) -> Option<String> {
    let s = v.as_str()?;
    let n = s.chars().count();
    ((1..=MAX_NAME).contains(&n) && !s.chars().any(char::is_control)).then(|| s.to_string())
}

fn names(v: &Value, max: usize) -> Option<Vec<String>> {
    let a = v.as_array()?;
    if a.len() > max {
        return None;
    }
    a.iter().map(name_of).collect()
}

fn setup_of(v: &Value) -> Result<PoolSetup, &'static str> {
    let bad = "setup_invalid";
    let state = v["state"].as_str().ok_or(bad)?;
    if !SETUP_STATES.contains(&state) {
        return Err(bad);
    }
    let members = v["members"].as_array().ok_or(bad)?;
    if members.len() > MAX_MEMBERS {
        return Err(bad);
    }
    let members = members
        .iter()
        .map(|m| {
            let status = m["status"]
                .as_str()
                .filter(|s| MEMBER_STATUSES.contains(s))?;
            let detail = m["detail"]
                .as_str()
                .filter(|d| d.chars().count() <= MAX_DETAIL)?;
            Some(SetupMember {
                name: name_of(&m["name"])?,
                status: status.into(),
                detail: detail.into(),
            })
        })
        .collect::<Option<Vec<_>>>()
        .ok_or(bad)?;
    Ok(PoolSetup {
        state: state.into(),
        members,
    })
}

fn fast_mode_of(v: &Value) -> Result<FastMode, &'static str> {
    let bad = "fast_mode_invalid";
    Ok(FastMode {
        available: v["available"].as_bool().ok_or(bad)?,
        missing_rdma: names(&v["missing_rdma"], MAX_MEMBERS).ok_or(bad)?,
    })
}

/// The `pool` object of a `/health` body, or the fixed reason it was refused.
fn pool_of(v: &Value) -> Result<Pool, &'static str> {
    let backend = v["backend"].as_str().ok_or("backend_invalid")?;
    if !["jaccl", "ring"].contains(&backend) {
        return Err("backend_invalid");
    }
    let mode = v["mode"].as_str().ok_or("mode_invalid")?;
    if !["tensor", "pipeline"].contains(&mode) {
        return Err("mode_invalid");
    }
    let setup = match v.get("setup").filter(|s| !s.is_null()) {
        Some(s) => Some(setup_of(s)?),
        None => None,
    };
    let fast_mode = match v.get("fast_mode").filter(|s| !s.is_null()) {
        Some(f) => Some(fast_mode_of(f)?),
        None => None,
    };
    let early = setup
        .as_ref()
        .is_some_and(|s| EARLY_STATES.contains(&s.state.as_str()));
    let raw = v["members"].as_array().ok_or("members_invalid")?;
    let min = if early { 1 } else { 2 };
    if raw.len() < min || raw.len() > MAX_MEMBERS {
        return Err("members_invalid");
    }
    let members = raw
        .iter()
        .map(|m| {
            Some(PoolMember {
                name: name_of(&m["name"])?,
                memory_total_bytes: m["memory_total_bytes"].as_u64()?,
            })
        })
        .collect::<Option<Vec<_>>>()
        .ok_or("members_invalid")?;
    Ok(Pool {
        members,
        backend: backend.into(),
        mode: mode.into(),
        setup,
        fast_mode,
    })
}

/// Pure: what a `/health` answer (status code and parsed body) says.
pub fn interpret(status: u16, body: &Value) -> (PoolProbe, Option<&'static str>) {
    let mut probe = PoolProbe {
        healthy: status == 200 && body["status"] == "ok",
        memory_total_bytes: body["memory_total_bytes"].as_u64(),
        memory_free_bytes: body["memory_free_bytes"].as_u64(),
        max_concurrency: body["max_concurrency"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok()),
        pool: None,
    };
    match pool_of(&body["pool"]) {
        Ok(p) => {
            probe.pool = Some(p);
            (probe, None)
        }
        Err(why) => {
            // A refused body contributes nothing: no pool, no numbers.
            (
                PoolProbe {
                    healthy: false,
                    ..PoolProbe::default()
                },
                Some(why),
            )
        }
    }
}

/// `GET <base>/health`. Never fails: anything unreadable is an unhealthy advert
/// without a pool.
pub async fn pool_advert(client: &reqwest::Client, base: &Url) -> PoolProbe {
    let Ok(url) = base.join("/health") else {
        return PoolProbe::default();
    };
    let Ok(resp) = client.get(url).timeout(PROBE_TIMEOUT).send().await else {
        return PoolProbe::default();
    };
    let status = resp.status().as_u16();
    let Ok(body) = resp.json::<Value>().await else {
        return PoolProbe::default();
    };
    let (probe, refused) = interpret(status, &body);
    if let Some(why) = refused {
        super::log::event("pool_health_refused", &[("reason", why.to_string())]);
    }
    probe
}

/// Fold a pool probe into the advert built from the local facts and `/v1/models`:
/// the pool's own memory and concurrency replace the local machine's, and the
/// advert is healthy only when both the pool and the model list answer.
pub fn apply(mut advert: Advert, p: PoolProbe) -> Advert {
    advert.healthy = advert.healthy && p.healthy;
    advert.pool = p.pool;
    if let Some(n) = p.memory_total_bytes {
        advert.memory_total_bytes = n;
    }
    if let Some(n) = p.memory_free_bytes {
        advert.memory_free_bytes = n;
    }
    if let Some(n) = p.max_concurrency {
        advert.max_concurrency = n;
    }
    advert
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::config::{MachineConfig, ServerKind};
    use crate::machine::{advert::SystemFacts, probe};
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn health(status: &str, setup_state: &str) -> Value {
        json!({
            "status": status, "model": "m", "backend": "jaccl", "mode": "tensor",
            "max_concurrency": 4,
            "memory_total_bytes": 1000u64, "memory_free_bytes": 600u64,
            "pool": {
                "members": [
                    {"name": "macA", "memory_total_bytes": 500u64},
                    {"name": "macB", "memory_total_bytes": 500u64}],
                "backend": "jaccl", "mode": "tensor",
                "setup": {"state": setup_state, "members": [
                    {"name": "macA", "status": "ready", "detail": ""},
                    {"name": "macB", "status": "ready", "detail": ""}]},
                "fast_mode": {"available": true, "missing_rdma": ["macB"]}
            }
        })
    }

    async fn serve(code: u16, body: &Value) -> (MockServer, Url) {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(code).set_body_json(body))
            .mount(&s)
            .await;
        let u = Url::parse(&s.uri()).unwrap();
        (s, u)
    }

    async fn probe_of(code: u16, body: &Value) -> PoolProbe {
        let (_s, u) = serve(code, body).await;
        pool_advert(&probe::client(), &u).await
    }

    #[tokio::test]
    async fn pool_health_ok_fills_pool_and_summed_memory() {
        let p = probe_of(200, &health("ok", "serving")).await;
        assert!(p.healthy);
        assert_eq!(
            (p.memory_total_bytes, p.memory_free_bytes, p.max_concurrency),
            (Some(1000), Some(600), Some(4))
        );
        let pool = p.pool.unwrap();
        assert_eq!(pool.members.len(), 2);
        assert_eq!(
            (pool.backend.as_str(), pool.mode.as_str()),
            ("jaccl", "tensor")
        );
        let facts = SystemFacts {
            memory_total_bytes: 1,
            memory_free_bytes: 1,
            accelerator: "apple-m3-ultra".into(),
        };
        let cfg = MachineConfig {
            server: ServerKind::Quexo,
            url: "http://127.0.0.1:52415".into(),
            max_concurrency: 2,
            api_key_file: None,
        };
        let base = probe::advert(
            &facts,
            &probe::Probe {
                healthy: true,
                ..Default::default()
            },
            &cfg,
        );
        let a = apply(base, probe_of(200, &health("ok", "serving")).await);
        assert_eq!(
            (
                a.server.as_str(),
                a.healthy,
                a.memory_total_bytes,
                a.memory_free_bytes,
                a.max_concurrency
            ),
            ("quexo", true, 1000, 600, 4)
        );
        assert!(a.pool.is_some());
    }

    #[tokio::test]
    async fn pool_health_starting_is_unhealthy_with_pool_kept() {
        let p = probe_of(503, &health("starting", "installing")).await;
        assert!(!p.healthy);
        assert!(p.pool.is_some());
    }

    #[tokio::test]
    async fn degraded_pool_is_unhealthy() {
        let p = probe_of(503, &health("degraded", "degraded")).await;
        assert!(!p.healthy);
        assert!(p.pool.is_some());
        // a 200 that says otherwise is still not healthy
        assert!(!probe_of(200, &health("degraded", "degraded")).await.healthy);
    }

    #[tokio::test]
    async fn pool_setup_and_fast_mode_are_passed_through() {
        let pool = probe_of(200, &health("ok", "serving")).await.pool.unwrap();
        assert_eq!(
            pool.setup,
            Some(PoolSetup {
                state: "serving".into(),
                members: vec![
                    SetupMember {
                        name: "macA".into(),
                        status: "ready".into(),
                        detail: "".into()
                    },
                    SetupMember {
                        name: "macB".into(),
                        status: "ready".into(),
                        detail: "".into()
                    }
                ]
            })
        );
        assert_eq!(
            pool.fast_mode,
            Some(FastMode {
                available: true,
                missing_rdma: vec!["macB".into()]
            })
        );
    }

    #[tokio::test]
    async fn pool_in_pairing_with_one_member_is_kept() {
        let mut h = health("starting", "pairing");
        h["pool"]["members"] = json!([{"name": "macA", "memory_total_bytes": 500u64}]);
        let p = probe_of(503, &h).await;
        assert_eq!(p.pool.unwrap().members.len(), 1);
        assert!(!p.healthy);
    }

    #[tokio::test]
    async fn pool_serving_with_one_member_is_dropped() {
        let mut h = health("ok", "serving");
        h["pool"]["members"] = json!([{"name": "macA", "memory_total_bytes": 500u64}]);
        let p = probe_of(200, &h).await;
        assert!(p.pool.is_none());
        assert!(!p.healthy);
    }

    #[tokio::test]
    async fn setup_with_unknown_status_drops_pool() {
        let mut h = health("ok", "serving");
        h["pool"]["setup"]["members"][0]["status"] = json!("exploding");
        assert!(probe_of(200, &h).await.pool.is_none());
        let mut h = health("ok", "serving");
        h["pool"]["setup"]["state"] = json!("rebooting");
        assert!(probe_of(200, &h).await.pool.is_none());
        let mut h = health("ok", "serving");
        h["pool"]["setup"]["members"][0]["detail"] = json!("x".repeat(201));
        let p = probe_of(200, &h).await;
        assert!(p.pool.is_none() && !p.healthy);
    }

    #[tokio::test]
    async fn pool_with_six_members_is_dropped() {
        let mut h = health("ok", "serving");
        h["pool"]["members"] = json!((0..6)
            .map(|i| json!({"name": format!("m{i}"), "memory_total_bytes": 1u64}))
            .collect::<Vec<_>>());
        let p = probe_of(200, &h).await;
        assert!(p.pool.is_none() && !p.healthy);
    }

    #[tokio::test]
    async fn pool_with_unknown_backend_is_dropped() {
        let mut h = health("ok", "serving");
        h["pool"]["backend"] = json!("carrier-pigeon");
        assert!(probe_of(200, &h).await.pool.is_none());
        let mut h = health("ok", "serving");
        h["pool"]["mode"] = json!("sideways");
        assert!(probe_of(200, &h).await.pool.is_none());
    }

    #[test]
    fn names_are_one_to_sixty_four_printable_chars() {
        for bad in ["".to_string(), "a".repeat(65), "bad\nname".into()] {
            let mut h = health("ok", "serving");
            h["pool"]["members"][0]["name"] = json!(bad);
            assert!(interpret(200, &h).0.pool.is_none());
        }
    }

    #[tokio::test]
    async fn unreachable_server_is_an_unhealthy_advert_without_a_pool() {
        let s = MockServer::start().await;
        let u = Url::parse(&s.uri()).unwrap();
        drop(s);
        let p = pool_advert(&probe::client(), &u).await;
        assert_eq!(p, PoolProbe::default());
    }

    #[test]
    fn a_refusal_reason_is_a_fixed_string_so_no_body_text_reaches_the_log() {
        // The log is stderr, so this checks the guarantee by construction: a refusal carries only a fixed reason: the type is `&'static str`, so no body
        // text can reach the log line.
        let mut h = health("ok", "serving");
        h["pool"]["backend"] = json!("SECRET-MARKER");
        h["pool"]["members"][0]["name"] = json!("SECRET-MARKER");
        let (_, why) = interpret(200, &h);
        let why = why.expect("refused");
        let line =
            crate::machine::log::event_line("pool_health_refused", &[("reason", why.to_string())]);
        assert!(!line.contains("SECRET-MARKER"), "{line}");
        assert_eq!(line, "models: pool_health_refused reason=backend_invalid");
    }

    #[test]
    fn quexo_default_url_is_loopback_52415() {
        assert_eq!(ServerKind::Quexo.default_url(), "http://127.0.0.1:52415");
    }

    #[test]
    fn non_quexo_server_advert_is_unchanged() {
        let facts = SystemFacts {
            memory_total_bytes: 64,
            memory_free_bytes: 32,
            accelerator: "apple-m3-ultra".into(),
        };
        let cfg = MachineConfig {
            server: ServerKind::Ollama,
            url: "http://127.0.0.1:11434".into(),
            max_concurrency: 4,
            api_key_file: None,
        };
        let a = probe::advert(
            &facts,
            &probe::Probe {
                healthy: true,
                models: vec![crate::machine::protocol::AdvertModel {
                    id: "qwen3:32b".into(),
                    context_length: None,
                }],
                ..Default::default()
            },
            &cfg,
        );
        // Byte-identical to what P2's advert serialised before pools had set-up fields.
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            r#"{"server":"ollama","models":[{"id":"qwen3:32b"}],"memory_total_bytes":64,"memory_free_bytes":32,"accelerator":"apple-m3-ultra","max_concurrency":4,"healthy":true,"pool":null}"#
        );
        // And a pool without set-up fields serialises as P2's did.
        let p = Pool {
            members: vec![PoolMember {
                name: "a".into(),
                memory_total_bytes: 1,
            }],
            backend: "ring".into(),
            mode: "pipeline".into(),
            setup: None,
            fast_mode: None,
        };
        assert_eq!(
            serde_json::to_string(&p).unwrap(),
            r#"{"members":[{"name":"a","memory_total_bytes":1}],"backend":"ring","mode":"pipeline"}"#
        );
    }

    fn dropped(h: &Value) -> bool {
        let (p, why) = interpret(200, h);
        p.pool.is_none() && !p.healthy && why.is_some()
    }

    fn kept(status: u16, h: &Value) -> bool {
        interpret(status, h).0.pool.is_some()
    }

    fn members(n: usize) -> Value {
        json!((0..n)
            .map(|i| json!({"name": format!("m{i}"), "memory_total_bytes": 1u64}))
            .collect::<Vec<_>>())
    }

    #[test]
    fn fast_mode_is_checked() {
        let mut h = health("ok", "serving");
        h["pool"]["fast_mode"]["available"] = json!("yes");
        assert!(dropped(&h));
        let mut h = health("ok", "serving");
        h["pool"]["fast_mode"]["missing_rdma"] = json!(["a", "b", "c", "d", "e", "f"]);
        assert!(dropped(&h));
        let mut h = health("ok", "serving");
        h["pool"]["fast_mode"]["missing_rdma"] = json!(["ok", "bad\tname"]);
        assert!(dropped(&h));
        let mut h = health("ok", "serving");
        h["pool"]["fast_mode"]["missing_rdma"] = json!(["a", "b", "c", "d", "e"]);
        assert!(kept(200, &h));
    }

    #[test]
    fn setup_members_are_checked() {
        let mut h = health("ok", "serving");
        h["pool"]["setup"]["members"] = json!((0..6)
            .map(|i| json!({"name": format!("m{i}"), "status": "ready", "detail": ""}))
            .collect::<Vec<_>>());
        assert!(dropped(&h));
        let mut h = health("ok", "serving");
        h["pool"]["setup"]["members"][0]["name"] = json!("");
        assert!(dropped(&h));
        let mut h = health("ok", "serving");
        h["pool"]["setup"]["members"][1]["name"] = json!("a".repeat(65));
        assert!(dropped(&h));
    }

    #[test]
    fn member_count_depends_on_the_setup_state() {
        for state in ["installing", "checking", "pairing"] {
            let mut h = health("starting", state);
            h["pool"]["members"] = members(1);
            assert!(kept(503, &h), "1 member in {state}");
            h["pool"]["members"] = members(0);
            assert!(dropped(&h), "0 members in {state}");
            h["pool"]["members"] = members(6);
            assert!(dropped(&h), "6 members in {state}");
            h["pool"]["members"] = members(5);
            assert!(kept(503, &h), "5 members in {state}");
        }
        let mut h = health("degraded", "degraded");
        h["pool"]["members"] = members(1);
        assert!(dropped(&h));
        let mut h = health("ok", "serving");
        h["pool"]["members"] = members(2);
        assert!(kept(200, &h));
        h["pool"]["members"] = members(5);
        assert!(kept(200, &h));
    }

    #[test]
    fn boundaries_are_accepted() {
        let mut h = health("ok", "serving");
        h["pool"]["members"][0]["name"] = json!("a".repeat(64));
        h["pool"]["setup"]["members"][0]["detail"] = json!("d".repeat(200));
        let (p, why) = interpret(200, &h);
        assert!(why.is_none() && p.healthy);
        assert_eq!(p.pool.unwrap().members[0].name.len(), 64);
    }

    #[test]
    fn a_refused_body_contributes_no_numbers() {
        let mut h = health("ok", "serving");
        h["pool"]["backend"] = json!("nope");
        let (p, why) = interpret(200, &h);
        assert!(why.is_some());
        assert_eq!(p, PoolProbe::default());
        // So the advert keeps the local machine's own numbers and is unhealthy.
        let base = Advert {
            server: "quexo".into(),
            models: vec![],
            memory_total_bytes: 7,
            memory_free_bytes: 3,
            accelerator: "cpu".into(),
            max_concurrency: 2,
            healthy: true,
            pool: None,
        };
        let a = apply(base, p);
        assert_eq!(
            (
                a.healthy,
                a.memory_total_bytes,
                a.memory_free_bytes,
                a.max_concurrency
            ),
            (false, 7, 3, 2)
        );
    }
}
