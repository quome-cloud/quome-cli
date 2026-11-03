use super::state::write_private;
use crate::errors::QuomeError;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

/// Keychain service name for the agent key on macOS.
pub const KEYCHAIN_SERVICE: &str = "cloud.quome.agent";

pub struct AgentKey {
    key: SigningKey,
}

/// Where the private key and VRC live. Injected so tests never touch the
/// real Keychain or `~/.quome`.
pub trait KeyStore {
    fn put(&self, profile: &str, secret: &str) -> Result<(), QuomeError>;
    fn get(&self, profile: &str) -> Result<String, QuomeError>;
}

/// `<dir>/cloud-<profile>.key`, mode 0600 (the Linux backend).
pub struct FileStore {
    pub dir: PathBuf,
}

impl FileStore {
    fn path(&self, profile: &str) -> Result<PathBuf, QuomeError> {
        let ok = !profile.is_empty()
            && profile
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
            && !profile.starts_with('.');
        if !ok {
            return Err(QuomeError::CloudKey(format!(
                "invalid profile name \"{profile}\""
            )));
        }
        Ok(self.dir.join(format!("cloud-{profile}.key")))
    }
}

impl KeyStore for FileStore {
    fn put(&self, profile: &str, secret: &str) -> Result<(), QuomeError> {
        write_private(&self.path(profile)?, secret.as_bytes())
    }
    fn get(&self, profile: &str) -> Result<String, QuomeError> {
        std::fs::read_to_string(self.path(profile)?)
            .map_err(|e| QuomeError::CloudKey(format!("cannot read key for \"{profile}\": {e}")))
    }
}

#[cfg(target_os = "macos")]
pub struct KeychainStore;

#[cfg(target_os = "macos")]
impl KeychainStore {
    fn entry(profile: &str) -> Result<keyring::Entry, QuomeError> {
        keyring::Entry::new(KEYCHAIN_SERVICE, profile)
            .map_err(|e| QuomeError::CloudKey(e.to_string()))
    }
}

#[cfg(target_os = "macos")]
impl KeyStore for KeychainStore {
    fn put(&self, profile: &str, secret: &str) -> Result<(), QuomeError> {
        Self::entry(profile)?
            .set_password(secret)
            .map_err(|e| QuomeError::CloudKey(e.to_string()))
    }
    fn get(&self, profile: &str) -> Result<String, QuomeError> {
        Self::entry(profile)?
            .get_password()
            .map_err(|e| QuomeError::CloudKey(format!("no key for \"{profile}\": {e}")))
    }
}

/// The platform backend: Keychain on macOS, a 0600 file on Linux, and
/// unsupported on Windows.
pub fn platform_store() -> Result<Box<dyn KeyStore>, QuomeError> {
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(KeychainStore))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let home = dirs::home_dir()
            .ok_or_else(|| QuomeError::CloudKey("cannot find home directory".into()))?;
        Ok(Box::new(FileStore {
            dir: super::state::quome_dir(&home),
        }))
    }
    #[cfg(not(unix))]
    {
        Err(QuomeError::CloudUnsupported)
    }
}

impl AgentKey {
    pub fn generate() -> AgentKey {
        AgentKey {
            key: SigningKey::generate(&mut rand_core::OsRng),
        }
    }

    pub fn from_seed(seed: [u8; 32]) -> AgentKey {
        AgentKey {
            key: SigningKey::from_bytes(&seed),
        }
    }

    /// Public key `x`, base64url without padding (43 chars).
    pub fn x_b64(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.key.verifying_key().as_bytes())
    }

    /// RFC 7638 JWK thumbprint (SHA-256, base64url).
    pub fn jkt(&self) -> String {
        let canon = format!(r#"{{"crv":"Ed25519","kty":"OKP","x":"{}"}}"#, self.x_b64());
        URL_SAFE_NO_PAD.encode(Sha256::digest(canon.as_bytes()))
    }

    pub fn sign(&self, msg: &[u8]) -> [u8; 64] {
        self.key.sign(msg).to_bytes()
    }

    pub fn store_in(&self, s: &dyn KeyStore, profile: &str, vrc: &str) -> Result<(), QuomeError> {
        let blob = serde_json::json!({
            "seed": URL_SAFE_NO_PAD.encode(self.key.to_bytes()),
            "vrc": vrc,
        });
        s.put(profile, &blob.to_string())
    }

    pub fn load_from(s: &dyn KeyStore, profile: &str) -> Result<(AgentKey, String), QuomeError> {
        let bad = || QuomeError::CloudKey(format!("stored key for \"{profile}\" is corrupt"));
        let v: serde_json::Value = serde_json::from_str(&s.get(profile)?).map_err(|_| bad())?;
        let seed = URL_SAFE_NO_PAD
            .decode(v["seed"].as_str().ok_or_else(bad)?)
            .map_err(|_| bad())?;
        let seed: [u8; 32] = seed.try_into().map_err(|_| bad())?;
        let vrc = v["vrc"].as_str().ok_or_else(bad)?.to_string();
        Ok((AgentKey::from_seed(seed), vrc))
    }

    /// Store via the platform backend (Keychain / Linux file).
    pub fn store(&self, profile: &str, vrc: &str) -> Result<(), QuomeError> {
        self.store_in(platform_store()?.as_ref(), profile, vrc)
    }

    /// Load via the platform backend.
    pub fn load(profile: &str) -> Result<(AgentKey, String), QuomeError> {
        Self::load_from(platform_store()?.as_ref(), profile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn b64(s: &str) -> Vec<u8> {
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(s)
            .unwrap()
    }

    // RFC 8037 Appendix A.1 Ed25519 key; thumbprint kPrK_qmx... is the RFC 8037 A.3 value, and was
    // re-derived independently with:
    //   python3 -c "import hashlib,base64;x='11qY...URo';print(base64.urlsafe_b64encode(hashlib.sha256(
    //   ('{\"crv\":\"Ed25519\",\"kty\":\"OKP\",\"x\":\"%s\"}'%x).encode()).digest()).rstrip(b'=').decode())"
    const D: &str = "nWGxne_9WmC6hEr0kuwsxERJxWl7MmkZcDusAxyuf2A";
    const X: &str = "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo";
    const JKT: &str = "kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k";

    fn rfc_key() -> AgentKey {
        AgentKey::from_seed(b64(D).try_into().unwrap())
    }

    #[test]
    fn x_and_jkt_match_rfc_vector() {
        let k = rfc_key();
        assert_eq!(k.x_b64(), X);
        assert_eq!(k.x_b64().len(), 43);
        assert_eq!(k.jkt(), JKT);
    }

    #[test]
    fn sign_verifies() {
        use ed25519_dalek::{Signature, Verifier, VerifyingKey};
        let k = rfc_key();
        let sig = k.sign(b"hello");
        let vk = VerifyingKey::from_bytes(&b64(X).try_into().unwrap()).unwrap();
        vk.verify(b"hello", &Signature::from_bytes(&sig)).unwrap();
        assert!(vk.verify(b"hellp", &Signature::from_bytes(&sig)).is_err());
    }

    #[test]
    fn generate_is_random() {
        assert_ne!(AgentKey::generate().x_b64(), AgentKey::generate().x_b64());
    }

    #[test]
    fn file_store_round_trip_and_mode() {
        let home = tempfile::tempdir().unwrap();
        let store = FileStore {
            dir: home.path().join(".quome"),
        };
        let k = AgentKey::generate();
        k.store_in(&store, "work", "vrc-token").unwrap();
        let (k2, vrc) = AgentKey::load_from(&store, "work").unwrap();
        assert_eq!(k2.x_b64(), k.x_b64());
        assert_eq!(vrc, "vrc-token");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = std::fs::metadata(home.path().join(".quome/cloud-work.key"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(m & 0o777, 0o600);
        }
        assert!(AgentKey::load_from(&store, "other").is_err());
    }

    #[test]
    fn file_store_rejects_path_traversal_profile() {
        let home = tempfile::tempdir().unwrap();
        let store = FileStore {
            dir: home.path().to_path_buf(),
        };
        assert!(AgentKey::generate().store_in(&store, "../x", "v").is_err());
    }
}
