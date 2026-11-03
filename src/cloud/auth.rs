//! RFC 8693 token exchange with quauth: the agent signs a short-lived actor
//! token with its key and trades it, plus the VRC, for a plain bearer token.
use super::keys::AgentKey;
use super::state::Profile;
use crate::errors::QuomeError;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:token-exchange";
const VRC_TYPE: &str = "urn:quauth:params:oauth:token-type:vrc";
const JWT_TYPE: &str = "urn:ietf:params:oauth:token-type:jwt";
/// Refresh this long before the token expires.
const SKEW: Duration = Duration::from_secs(30);

pub struct TokenSource {
    http: reqwest::Client,
    issuer: String,
    resource: String,
    principal: String,
    kid: String,
    key: AgentKey,
    vrc: String,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    token_endpoint: Option<String>,
    cached: Option<(String, Instant)>,
}

impl TokenSource {
    pub fn new(profile: &Profile, key: AgentKey, vrc: String) -> TokenSource {
        TokenSource {
            http: reqwest::Client::new(),
            issuer: profile.issuer.trim_end_matches('/').to_string(),
            resource: format!("{}/", profile.api_url.trim_end_matches('/')),
            principal: profile.agent_principal.clone(),
            kid: profile.kid.clone(),
            key,
            vrc,
            state: Mutex::new(State::default()),
        }
    }

    /// A valid access token, exchanging anew when the cached one is within 30 s of expiry.
    pub async fn access_token(&self) -> Result<String, QuomeError> {
        let mut st = self.state.lock().await;
        if let Some((tok, exp)) = &st.cached {
            if Instant::now() < *exp {
                return Ok(tok.clone());
            }
        }
        let endpoint = match &st.token_endpoint {
            Some(e) => e.clone(),
            None => {
                let e = self.discover().await?;
                st.token_endpoint = Some(e.clone());
                e
            }
        };
        let jwt = self.actor_token(&endpoint);
        let resp = self
            .http
            .post(&endpoint)
            .form(&[
                ("grant_type", GRANT_TYPE),
                ("subject_token", self.vrc.as_str()),
                ("subject_token_type", VRC_TYPE),
                ("actor_token", jwt.as_str()),
                ("actor_token_type", JWT_TYPE),
                ("resource", self.resource.as_str()),
            ])
            .send()
            .await?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        if !status.is_success() {
            return Err(map_exchange_error(status.as_u16(), &v));
        }
        let token = v["access_token"]
            .as_str()
            .ok_or(QuomeError::InvalidResponse)?
            .to_string();
        let expires_in = v["expires_in"].as_u64().unwrap_or(0);
        let ttl = Duration::from_secs(expires_in).saturating_sub(SKEW);
        st.cached = Some((token.clone(), Instant::now() + ttl));
        Ok(token)
    }

    async fn discover(&self) -> Result<String, QuomeError> {
        let url = format!("{}/.well-known/openid-configuration", self.issuer);
        let resp = self.http.get(&url).send().await?;
        if !resp.status().is_success() {
            return Err(QuomeError::ApiError(format!(
                "Quome Cloud sign-in discovery failed ({})",
                resp.status()
            )));
        }
        let v: serde_json::Value = resp.json().await?;
        v["token_endpoint"]
            .as_str()
            .map(str::to_string)
            .ok_or(QuomeError::InvalidResponse)
    }

    /// The signed actor-token JWT (EdDSA), audience = the token endpoint.
    fn actor_token(&self, audience: &str) -> String {
        let iat = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let header = serde_json::json!({"alg": "EdDSA", "typ": "JWT", "kid": self.kid});
        let claims = serde_json::json!({
            "iss": self.principal,
            "sub": self.principal,
            "aud": audience,
            "iat": iat,
            "exp": iat + 120,
            "jti": uuid::Uuid::new_v4().to_string(),
        });
        let signing_input = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(header.to_string()),
            URL_SAFE_NO_PAD.encode(claims.to_string())
        );
        let sig = self.key.sign(signing_input.as_bytes());
        format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(sig))
    }
}

fn map_exchange_error(status: u16, v: &serde_json::Value) -> QuomeError {
    if status == 401 {
        match v["reason"].as_str() {
            Some("delegation_revoked") => return QuomeError::CloudDisconnected,
            Some("agent_not_connected") => return QuomeError::CloudNotYetConnected,
            _ => {}
        }
    }
    if status == 400 && v["error"].as_str() == Some("invalid_grant") {
        // revoked, expired, or the key no longer matches the delegation
        return QuomeError::CloudDisconnected;
    }
    let err = v["error"].as_str().unwrap_or("unknown_error");
    QuomeError::ApiError(format!(
        "Quome Cloud rejected the token exchange ({status} {err})"
    ))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    pub(crate) fn profile(server: &str) -> Profile {
        Profile {
            edition: "cloud".into(),
            api_url: server.into(),
            console_url: server.into(),
            issuer: server.into(),
            agent_name: "a".into(),
            agent_principal: "agent:abc".into(),
            kid: "kid-1".into(),
            jkt: "j".into(),
            delegation_id: "d".into(),
            expires_at: "2030-01-01T00:00:00Z".into(),
            org_id: "11111111-1111-1111-1111-111111111111".into(),
            org_slug: "acme".into(),
            apps_domain: "apps.example".into(),
        }
    }

    async fn mount_discovery(s: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"token_endpoint": format!("{}/oauth/token", s.uri())}),
            ))
            .mount(s)
            .await;
    }

    fn token_resp(expires_in: u64) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "AT", "token_type": "Bearer", "expires_in": expires_in,
            "scope": "x", "issued_token_type": "urn:ietf:params:oauth:token-type:access_token"
        }))
    }

    #[tokio::test]
    async fn exchange_form_and_actor_jwt() {
        let s = MockServer::start().await;
        mount_discovery(&s).await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(token_resp(3600))
            .expect(1)
            .mount(&s)
            .await;
        let key = AgentKey::generate();
        let x = key.x_b64();
        let ts = TokenSource::new(&profile(&s.uri()), key, "the-vrc".into());
        assert_eq!(ts.access_token().await.unwrap(), "AT");

        let reqs = s.received_requests().await.unwrap();
        let req = reqs
            .iter()
            .find(|r| r.url.path() == "/oauth/token")
            .unwrap();
        let body = String::from_utf8(req.body.clone()).unwrap();
        let form: std::collections::HashMap<String, String> =
            reqwest::Url::parse(&format!("http://x/?{body}"))
                .unwrap()
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
        assert_eq!(form["grant_type"], GRANT_TYPE);
        assert_eq!(form["subject_token"], "the-vrc");
        assert_eq!(form["subject_token_type"], VRC_TYPE);
        assert_eq!(form["actor_token_type"], JWT_TYPE);
        assert_eq!(form["resource"], format!("{}/", s.uri()));
        assert_eq!(form.len(), 6);

        let jwt = &form["actor_token"];
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let dec = |p: &str| -> serde_json::Value {
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(p).unwrap()).unwrap()
        };
        assert_eq!(
            dec(parts[0]),
            serde_json::json!({"alg":"EdDSA","typ":"JWT","kid":"kid-1"})
        );
        let c = dec(parts[1]);
        assert_eq!(c["iss"], "agent:abc");
        assert_eq!(c["sub"], "agent:abc");
        assert_eq!(c["aud"], format!("{}/oauth/token", s.uri()));
        let (iat, exp) = (c["iat"].as_u64().unwrap(), c["exp"].as_u64().unwrap());
        assert_eq!(exp - iat, 120);
        assert!(uuid::Uuid::parse_str(c["jti"].as_str().unwrap()).is_ok());
        let vk = VerifyingKey::from_bytes(&URL_SAFE_NO_PAD.decode(x).unwrap().try_into().unwrap())
            .unwrap();
        let sig = Signature::from_bytes(
            &URL_SAFE_NO_PAD
                .decode(parts[2])
                .unwrap()
                .try_into()
                .unwrap(),
        );
        vk.verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig)
            .unwrap();
    }

    #[tokio::test]
    async fn token_cached_until_30s_before_expiry() {
        let s = MockServer::start().await;
        mount_discovery(&s).await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(token_resp(3600))
            .expect(1)
            .mount(&s)
            .await;
        let ts = TokenSource::new(&profile(&s.uri()), AgentKey::generate(), "v".into());
        ts.access_token().await.unwrap();
        ts.access_token().await.unwrap();
    }

    #[tokio::test]
    async fn token_inside_skew_window_is_refetched() {
        let s = MockServer::start().await;
        mount_discovery(&s).await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(token_resp(20))
            .expect(2)
            .mount(&s)
            .await;
        let ts = TokenSource::new(&profile(&s.uri()), AgentKey::generate(), "v".into());
        ts.access_token().await.unwrap();
        ts.access_token().await.unwrap();
    }

    #[tokio::test]
    async fn invalid_grant_is_disconnected() {
        let s = MockServer::start().await;
        mount_discovery(&s).await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(
                ResponseTemplate::new(400)
                    .set_body_json(serde_json::json!({"error":"invalid_grant"})),
            )
            .mount(&s)
            .await;
        let ts = TokenSource::new(&profile(&s.uri()), AgentKey::generate(), "v".into());
        let e = ts.access_token().await.unwrap_err();
        assert!(matches!(e, QuomeError::CloudDisconnected), "{e}");
    }
}
