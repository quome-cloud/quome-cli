//! Wire types of the models tunnel: control-stream JSON lines between this
//! machine agent and the platform's gateway, the advert, and the per-request
//! token verifier. Checked against `tests/vectors/models-tunnel-v1.json`,
//! which both ends commit byte-for-byte.

use std::collections::HashMap;
use std::fmt;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ed25519_dalek::{Signature, VerifyingKey};
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;

pub const MAX_LIFETIME_SECS: i64 = 60;
pub const SKEW_SECS: i64 = 30;
const MAX_REPLAY_ENTRIES: usize = 100_000;

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("malformed control line: {0}")]
    Line(String),
    #[error("malformed gateway key")]
    Key,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AdvertModel {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub context_length: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PoolMember {
    pub name: String,
    pub memory_total_bytes: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Pool {
    pub members: Vec<PoolMember>,
    pub backend: String,
    pub mode: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Advert {
    /// "ollama" | "vllm" | "llama.cpp" | "mlx" | "quexo"
    pub server: String,
    pub models: Vec<AdvertModel>,
    pub memory_total_bytes: u64,
    pub memory_free_bytes: u64,
    pub accelerator: String,
    pub max_concurrency: u32,
    pub healthy: bool,
    /// Serialised as null when absent.
    #[serde(default)]
    pub pool: Option<Pool>,
}

#[derive(Clone, PartialEq)]
pub enum ControlMsg {
    Hello {
        agent_version: String,
        advert: Advert,
    },
    Advert {
        advert: Advert,
    },
    Token {
        access_token: String,
    },
    Welcome {
        gateway_key: String,
        machine_id: String,
    },
    /// Any other `type`; never serialised.
    Unknown,
}

impl fmt::Debug for ControlMsg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ControlMsg::Hello {
                agent_version,
                advert,
            } => f
                .debug_struct("Hello")
                .field("agent_version", agent_version)
                .field("advert", advert)
                .finish(),
            ControlMsg::Advert { advert } => {
                f.debug_struct("Advert").field("advert", advert).finish()
            }
            ControlMsg::Token { .. } => f
                .debug_struct("Token")
                .field("access_token", &"<redacted>")
                .finish(),
            ControlMsg::Welcome {
                gateway_key,
                machine_id,
            } => f
                .debug_struct("Welcome")
                .field("gateway_key", gateway_key)
                .field("machine_id", machine_id)
                .finish(),
            ControlMsg::Unknown => f.write_str("Unknown"),
        }
    }
}

impl Serialize for ControlMsg {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            ControlMsg::Hello {
                agent_version,
                advert,
            } => {
                let mut m = s.serialize_map(Some(3))?;
                m.serialize_entry("type", "hello")?;
                m.serialize_entry("agent_version", agent_version)?;
                m.serialize_entry("advert", advert)?;
                m.end()
            }
            ControlMsg::Advert { advert } => {
                let mut m = s.serialize_map(Some(2))?;
                m.serialize_entry("type", "advert")?;
                m.serialize_entry("advert", advert)?;
                m.end()
            }
            ControlMsg::Token { access_token } => {
                let mut m = s.serialize_map(Some(2))?;
                m.serialize_entry("type", "token")?;
                m.serialize_entry("access_token", access_token)?;
                m.end()
            }
            ControlMsg::Welcome {
                gateway_key,
                machine_id,
            } => {
                let mut m = s.serialize_map(Some(3))?;
                m.serialize_entry("type", "welcome")?;
                m.serialize_entry("gateway_key", gateway_key)?;
                m.serialize_entry("machine_id", machine_id)?;
                m.end()
            }
            ControlMsg::Unknown => Err(serde::ser::Error::custom("unknown is never serialised")),
        }
    }
}

/// Compact JSON followed by a newline. `Unknown` is never sent; it encodes
/// as an empty line.
pub fn encode_line(m: &ControlMsg) -> String {
    let mut s = serde_json::to_string(m).unwrap_or_default();
    s.push('\n');
    s
}

fn field<T: serde::de::DeserializeOwned>(v: &Value, name: &str) -> Result<T, ProtocolError> {
    let x = v
        .get(name)
        .ok_or_else(|| ProtocolError::Line(format!("missing field {name}")))?;
    serde_json::from_value(x.clone()).map_err(|e| ProtocolError::Line(format!("{name}: {e}")))
}

/// Decode one control line. Unknown fields are ignored; an unknown `type`
/// decodes to `Unknown`.
pub fn decode_line(line: &str) -> Result<ControlMsg, ProtocolError> {
    let v: Value = serde_json::from_str(line.trim_end_matches(['\r', '\n']))
        .map_err(|e| ProtocolError::Line(e.to_string()))?;
    let ty = v
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| ProtocolError::Line("missing type".into()))?;
    Ok(match ty {
        "hello" => ControlMsg::Hello {
            agent_version: field(&v, "agent_version")?,
            advert: field(&v, "advert")?,
        },
        "advert" => ControlMsg::Advert {
            advert: field(&v, "advert")?,
        },
        "token" => ControlMsg::Token {
            access_token: field(&v, "access_token")?,
        },
        "welcome" => ControlMsg::Welcome {
            gateway_key: field(&v, "gateway_key")?,
            machine_id: field(&v, "machine_id")?,
        },
        _ => ControlMsg::Unknown,
    })
}

#[derive(Clone)]
pub struct GatewayKey(VerifyingKey);

impl GatewayKey {
    pub fn from_b64url(s: &str) -> Result<GatewayKey, ProtocolError> {
        let bytes = URL_SAFE_NO_PAD
            .decode(s.trim_end_matches('='))
            .map_err(|_| ProtocolError::Key)?;
        let arr: [u8; 32] = bytes.try_into().map_err(|_| ProtocolError::Key)?;
        VerifyingKey::from_bytes(&arr)
            .map(GatewayKey)
            .map_err(|_| ProtocolError::Key)
    }
}

impl fmt::Debug for GatewayKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("GatewayKey(..)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenRefusal {
    Malformed,
    BadAlg,
    BadSignature,
    WrongAudience,
    Expired,
    LifetimeTooLong,
    Replayed,
}

impl TokenRefusal {
    pub fn as_str(&self) -> &'static str {
        match self {
            TokenRefusal::Malformed => "malformed",
            TokenRefusal::BadAlg => "bad_alg",
            TokenRefusal::BadSignature => "bad_signature",
            TokenRefusal::WrongAudience => "wrong_audience",
            TokenRefusal::Expired => "expired",
            TokenRefusal::LifetimeTooLong => "lifetime_too_long",
            TokenRefusal::Replayed => "replayed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestClaims {
    pub rid: String,
    pub model: String,
    pub exp: i64,
}

pub struct RequestTokenVerifier {
    key: GatewayKey,
    audience: String,
    /// rid -> exp; entries live until `exp + SKEW_SECS`.
    seen: HashMap<String, i64>,
}

impl RequestTokenVerifier {
    pub fn new(key: GatewayKey, audience: String) -> Self {
        RequestTokenVerifier {
            key,
            audience,
            seen: HashMap::new(),
        }
    }

    pub fn verify(&mut self, token: &str, now: i64) -> Result<RequestClaims, TokenRefusal> {
        let parts: Vec<&str> = token.split('.').collect();
        let [h, p, sig] = parts[..] else {
            return Err(TokenRefusal::Malformed);
        };

        let header_bytes = URL_SAFE_NO_PAD
            .decode(h)
            .map_err(|_| TokenRefusal::Malformed)?;
        let header: Value =
            serde_json::from_slice(&header_bytes).map_err(|_| TokenRefusal::BadAlg)?;
        if header != serde_json::json!({"alg": "EdDSA", "typ": "JWT"}) {
            return Err(TokenRefusal::BadAlg);
        }

        let sig_bytes = URL_SAFE_NO_PAD
            .decode(sig)
            .map_err(|_| TokenRefusal::BadSignature)?;
        let signature =
            Signature::from_slice(&sig_bytes).map_err(|_| TokenRefusal::BadSignature)?;
        let signed = format!("{h}.{p}");
        self.key
            .0
            .verify_strict(signed.as_bytes(), &signature)
            .map_err(|_| TokenRefusal::BadSignature)?;

        let payload_bytes = URL_SAFE_NO_PAD
            .decode(p)
            .map_err(|_| TokenRefusal::Malformed)?;
        let claims: Value =
            serde_json::from_slice(&payload_bytes).map_err(|_| TokenRefusal::Malformed)?;
        let str_claim = |k: &str| claims.get(k).and_then(Value::as_str).map(str::to_owned);
        let (Some(aud), Some(rid), Some(model), Some(exp)) = (
            str_claim("aud"),
            str_claim("rid"),
            str_claim("model"),
            claims.get("exp").and_then(Value::as_i64),
        ) else {
            return Err(TokenRefusal::Malformed);
        };

        if aud != self.audience {
            return Err(TokenRefusal::WrongAudience);
        }
        if exp <= now - SKEW_SECS {
            return Err(TokenRefusal::Expired);
        }
        if exp > now + MAX_LIFETIME_SECS + SKEW_SECS {
            return Err(TokenRefusal::LifetimeTooLong);
        }
        if let Some(&seen_exp) = self.seen.get(&rid) {
            if seen_exp + SKEW_SECS > now {
                return Err(TokenRefusal::Replayed);
            }
        }
        self.seen.retain(|_, e| *e + SKEW_SECS > now);
        if self.seen.len() >= MAX_REPLAY_ENTRIES {
            return Err(TokenRefusal::Replayed);
        }
        self.seen.insert(rid.clone(), exp);
        Ok(RequestClaims { rid, model, exp })
    }
}

/// Exactly the three paths the tunnel serves; the path is compared
/// byte-for-byte with no normalisation.
pub fn path_allowed(method: &http::Method, path: &str) -> bool {
    matches!(
        (method.as_str(), path),
        ("POST", "/v1/chat/completions") | ("POST", "/v1/responses") | ("GET", "/v1/models")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    const VECTORS: &str = include_str!("../../tests/vectors/models-tunnel-v1.json");

    fn vectors() -> Value {
        serde_json::from_str(VECTORS).unwrap()
    }

    #[test]
    fn vectors_file_is_the_shared_one() {
        let hex: String = Sha256::digest(VECTORS.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            hex,
            "2f312afb3095e3e0453d53291bb765790100cdfef58dbb94446f400995c6e035"
        );
        assert_eq!(VECTORS.len(), 6966);
    }

    #[test]
    fn control_frames_round_trip() {
        let v = vectors();
        for f in v["control_frames"].as_array().unwrap() {
            let name = f["name"].as_str().unwrap();
            let line = f["line"].as_str().unwrap();
            let msg = decode_line(line).unwrap_or_else(|e| panic!("{name}: {e}"));
            if f.get("parse").and_then(Value::as_str) == Some("unknown") {
                assert_eq!(msg, ControlMsg::Unknown, "{name}");
                continue;
            }
            let variant = matches!(
                (&msg, name),
                (ControlMsg::Hello { .. }, "hello")
                    | (ControlMsg::Advert { .. }, "advert" | "advert_pool")
                    | (ControlMsg::Token { .. }, "token")
                    | (ControlMsg::Welcome { .. }, "welcome")
            );
            assert!(variant, "{name}: wrong variant {msg:?}");
            assert_eq!(encode_line(&msg), format!("{line}\n"), "{name}");
        }
    }

    #[test]
    fn decode_ignores_unknown_fields() {
        let m = decode_line(r#"{"type":"token","access_token":"t","extra":[1]}"#).unwrap();
        assert_eq!(
            m,
            ControlMsg::Token {
                access_token: "t".into()
            }
        );
        assert!(decode_line("not json").is_err());
    }

    fn verifier() -> (RequestTokenVerifier, i64, Value) {
        let v = vectors();
        let rt = &v["request_token"];
        let key = GatewayKey::from_b64url(rt["gateway_key"].as_str().unwrap()).unwrap();
        let ver = RequestTokenVerifier::new(key, rt["audience"].as_str().unwrap().to_string());
        (ver, rt["now"].as_i64().unwrap(), rt.clone())
    }

    #[test]
    fn request_token_cases() {
        let (mut ver, now, rt) = verifier();
        for c in rt["cases"].as_array().unwrap() {
            let name = c["name"].as_str().unwrap();
            let got = ver.verify(c["token"].as_str().unwrap(), now);
            if let Some(ok) = c["result"].get("ok") {
                let claims = got.unwrap_or_else(|e| panic!("{name}: {e:?}"));
                assert_eq!(claims.rid, ok["rid"].as_str().unwrap(), "{name}");
                assert_eq!(claims.model, ok["model"].as_str().unwrap(), "{name}");
            } else {
                let want = c["result"]["error"].as_str().unwrap();
                assert_eq!(got.unwrap_err().as_str(), want, "{name}");
            }
        }
    }

    #[test]
    fn a_rid_is_single_use() {
        let (mut ver, now, rt) = verifier();
        let tok = rt["cases"][0]["token"].as_str().unwrap().to_string();
        assert!(ver.verify(&tok, now).is_ok());
        assert_eq!(ver.verify(&tok, now), Err(TokenRefusal::Replayed));
        let (mut fresh, now, _) = verifier();
        assert_eq!(fresh.verify(&tok, now + 91), Err(TokenRefusal::Expired));
    }

    #[test]
    fn paths() {
        let v = vectors();
        let m = |s: &str| http::Method::from_bytes(s.as_bytes()).unwrap();
        for p in v["allowed_paths"].as_array().unwrap() {
            assert!(path_allowed(
                &m(p[0].as_str().unwrap()),
                p[1].as_str().unwrap()
            ));
        }
        for p in v["refused_paths"].as_array().unwrap() {
            assert!(!path_allowed(
                &m(p[0].as_str().unwrap()),
                p[1].as_str().unwrap()
            ));
        }
    }

    #[test]
    fn token_debug_redacts() {
        let s = format!(
            "{:?}",
            ControlMsg::Token {
                access_token: "secret".into()
            }
        );
        assert!(!s.contains("secret"));
    }
}
