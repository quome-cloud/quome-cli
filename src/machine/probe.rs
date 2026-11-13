//! Ask the local model server what it is serving. A failed probe is a fact
//! (`healthy: false`), never an error.
use std::time::Duration;

use super::advert::SystemFacts;
use super::config::{MachineConfig, ServerKind};
use super::protocol::{Advert, AdvertModel};

pub const OLLAMA_RESPONSES_MIN: (u64, u64, u64) = (0, 13, 3);
const CALL_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Probe {
    pub healthy: bool,
    pub models: Vec<AdvertModel>,
    pub ollama_version: Option<String>,
    pub responses_supported: bool,
}

/// The client probes use: loopback traffic never goes through a proxy.
pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("the probe client builds")
}

/// "0.13.3" and "1.0.0-rc1" parse; a pre-release or build suffix is ignored.
pub fn version_at_least(v: &str, min: (u64, u64, u64)) -> bool {
    let core = v.trim().split(['-', '+']).next().unwrap_or("");
    let mut it = core.split('.').map(|p| p.parse::<u64>());
    let mut next = || it.next().and_then(Result::ok);
    let Some(major) = next() else {
        return false;
    };
    (major, next().unwrap_or(0), next().unwrap_or(0)) >= min
}

async fn get_json(
    client: &reqwest::Client,
    url: String,
    bearer: &Option<String>,
) -> Option<serde_json::Value> {
    let mut req = client.get(url).timeout(CALL_TIMEOUT);
    if let Some(k) = bearer {
        req = req.bearer_auth(k);
    }
    let resp = req.send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json().await.ok()
}

fn ids(v: &serde_json::Value, list: &str, key: &str) -> Vec<AdvertModel> {
    v[list]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|m| m[key].as_str())
                .map(|id| AdvertModel {
                    id: id.to_string(),
                    context_length: None,
                })
                .collect()
        })
        .unwrap_or_default()
}

pub async fn probe(client: &reqwest::Client, cfg: &MachineConfig) -> Probe {
    // A configured key file that cannot be read means the server cannot be asked
    // properly: report unhealthy rather than probe without credentials.
    let bearer = match &cfg.api_key_file {
        Some(p) => match std::fs::read_to_string(p) {
            Ok(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
            _ => return Probe::default(),
        },
        None => None,
    };
    let base = cfg.url.trim_end_matches('/');
    if cfg.server == ServerKind::Ollama {
        let (ver, tags) = tokio::join!(
            get_json(client, format!("{base}/api/version"), &bearer),
            get_json(client, format!("{base}/api/tags"), &bearer),
        );
        let version = ver.and_then(|v| v["version"].as_str().map(String::from));
        let responses = version
            .as_deref()
            .is_some_and(|v| version_at_least(v, OLLAMA_RESPONSES_MIN));
        match tags {
            Some(t) => Probe {
                healthy: true,
                models: ids(&t, "models", "name"),
                ollama_version: version,
                responses_supported: responses,
            },
            None => Probe::default(),
        }
    } else {
        match get_json(client, format!("{base}/v1/models"), &bearer).await {
            Some(v) => Probe {
                healthy: true,
                models: ids(&v, "data", "id"),
                ollama_version: None,
                responses_supported: true,
            },
            None => Probe::default(),
        }
    }
}

pub fn advert(facts: &SystemFacts, p: &Probe, cfg: &MachineConfig) -> Advert {
    Advert {
        server: cfg.server.name().into(),
        models: p.models.clone(),
        memory_total_bytes: facts.memory_total_bytes,
        memory_free_bytes: facts.memory_free_bytes,
        accelerator: facts.accelerator.clone(),
        max_concurrency: cfg.max_concurrency,
        healthy: p.healthy,
        pool: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn cfg(server: ServerKind, url: String) -> MachineConfig {
        MachineConfig {
            server,
            url,
            max_concurrency: 4,
            api_key_file: None,
        }
    }

    #[tokio::test]
    async fn the_probe_client_does_not_follow_redirects() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/a"))
            .respond_with(ResponseTemplate::new(302).insert_header("location", "/b"))
            .mount(&s)
            .await;
        let r = client().get(format!("{}/a", s.uri())).send().await.unwrap();
        assert_eq!(r.status().as_u16(), 302);
        assert_eq!(s.received_requests().await.unwrap().len(), 1);
    }

    async fn ollama(s: &MockServer, version: &str, tags_status: u16) {
        Mock::given(method("GET"))
            .and(path("/api/version"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"version": version})),
            )
            .mount(s)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(
                ResponseTemplate::new(tags_status)
                    .set_body_json(serde_json::json!({"models": [{"name": "qwen3:32b"}]})),
            )
            .mount(s)
            .await;
    }

    #[tokio::test]
    async fn probe_ollama() {
        let s = MockServer::start().await;
        ollama(&s, "0.13.3", 200).await;
        let c = cfg(ServerKind::Ollama, s.uri());
        let p = probe(&client(), &c).await;
        assert!(p.healthy && p.responses_supported);
        assert_eq!(p.models.len(), 1);
        assert_eq!(p.models[0].id, "qwen3:32b");
        assert_eq!(p.ollama_version.as_deref(), Some("0.13.3"));

        let s = MockServer::start().await;
        ollama(&s, "0.12.9", 200).await;
        let p = probe(&client(), &cfg(ServerKind::Ollama, s.uri())).await;
        assert!(p.healthy && !p.responses_supported);

        let s = MockServer::start().await;
        ollama(&s, "0.13.3", 500).await;
        let p = probe(&client(), &cfg(ServerKind::Ollama, s.uri())).await;
        assert!(!p.healthy && p.models.is_empty());
    }

    #[tokio::test]
    async fn probe_openai_compatible() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header("authorization", "Bearer sekret"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"data": [{"id": "m"}]})),
            )
            .expect(1)
            .mount(&s)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("k");
        std::fs::write(&key, "sekret\n").unwrap();
        let mut c = cfg(ServerKind::Vllm, s.uri());
        c.api_key_file = Some(key);
        let p = probe(&client(), &c).await;
        assert!(p.healthy && p.responses_supported);
        assert_eq!(p.models[0].id, "m");

        c.api_key_file = Some(dir.path().join("missing"));
        assert!(!probe(&client(), &c).await.healthy);
    }

    #[tokio::test]
    async fn probe_times_out() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let _hold = tokio::spawn(async move {
            let mut socks = vec![];
            while let Ok((s, _)) = l.accept().await {
                socks.push(s);
            }
        });
        let t = std::time::Instant::now();
        let p = probe(&client(), &cfg(ServerKind::Vllm, url)).await;
        assert!(!p.healthy && p.models.is_empty());
        assert!(
            t.elapsed() < Duration::from_millis(2500),
            "{:?}",
            t.elapsed()
        );
    }

    #[test]
    fn versions() {
        let min = OLLAMA_RESPONSES_MIN;
        assert!(version_at_least("0.13.3", min));
        assert!(version_at_least("0.13.10", min));
        assert!(!version_at_least("0.13.2", min));
        assert!(version_at_least("1.0.0-rc1", min));
        assert!(!version_at_least("nonsense", min));
    }

    #[test]
    fn advert_carries_the_facts() {
        let facts = SystemFacts {
            memory_total_bytes: 10,
            memory_free_bytes: 4,
            accelerator: "cpu".into(),
        };
        let a = advert(
            &facts,
            &Probe::default(),
            &cfg(ServerKind::Mlx, "http://127.0.0.1:1".into()),
        );
        assert_eq!(
            (a.server.as_str(), a.healthy, a.max_concurrency),
            ("mlx", false, 4)
        );
        assert!(a.pool.is_none());
    }
}
