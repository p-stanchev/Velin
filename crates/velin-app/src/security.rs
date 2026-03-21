use anyhow::{Context, Result, anyhow, bail};
use getrandom::fill as random_fill;
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use x25519_dalek::{PublicKey, StaticSecret};

const SECURITY_CONTEXT: &[u8] = b"velin-session-v1";
const DEFAULT_TRUST_VALIDITY_DAYS: u32 = 30;

#[derive(Clone)]
pub struct LocalIdentity {
    secret: StaticSecret,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustedPeer {
    pub machine_name: String,
    pub public_key_hex: String,
    #[serde(default = "now_unix_secs")]
    pub trusted_at_unix_secs: u64,
}

#[derive(Debug, Clone)]
pub struct TrustedPeerView {
    pub machine_name: String,
    pub fingerprint: String,
    pub expires_in_days: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct SecurityStoreData {
    local_secret_hex: String,
    trusted_peers: Vec<TrustedPeer>,
    trust_validity_days: u32,
}

impl Default for SecurityStoreData {
    fn default() -> Self {
        Self {
            local_secret_hex: String::new(),
            trusted_peers: Vec::new(),
            trust_validity_days: DEFAULT_TRUST_VALIDITY_DAYS,
        }
    }
}

pub enum TrustOutcome {
    Trusted,
    Untrusted {
        machine_name: String,
    },
}

pub struct SecurityStore {
    path: PathBuf,
    data: SecurityStoreData,
}

impl SecurityStore {
    pub fn load_or_create() -> Result<Self> {
        let path = security_path()?;
        let data = if path.exists() {
            let bytes = fs::read(&path)
                .with_context(|| format!("failed to read security file {}", path.display()))?;
            serde_json::from_slice::<SecurityStoreData>(&bytes)
                .context("failed to parse security file")?
        } else {
            SecurityStoreData::default()
        };

        let mut store = Self { path, data };
        if store.data.local_secret_hex.trim().is_empty() {
            store.data.local_secret_hex = hex::encode(random_secret_bytes()?);
            store.save()?;
        }

        Ok(store)
    }

    pub fn local_identity(&self) -> Result<LocalIdentity> {
        Ok(LocalIdentity {
            secret: StaticSecret::from(secret_bytes_from_hex(&self.data.local_secret_hex)?),
        })
    }

    pub fn verify_peer(
        &mut self,
        machine_name: &str,
        public_key_hex: &str,
    ) -> Result<TrustOutcome> {
        let machine_name = machine_name.trim();
        if machine_name.is_empty() {
            bail!("peer machine name was empty");
        }

        let public_key_hex = public_key_hex.trim().to_lowercase();
        let _ = public_key_from_hex(&public_key_hex)?;
        self.prune_expired_peers();

        if let Some(peer) = self
            .data
            .trusted_peers
            .iter_mut()
            .find(|peer| peer.machine_name == machine_name)
        {
            if peer.public_key_hex == public_key_hex {
                return Ok(TrustOutcome::Trusted);
            }

            bail!(
                "trusted peer {machine_name} presented a different identity key; pairing refused"
            );
        }

        if let Some(peer) = self
            .data
            .trusted_peers
            .iter_mut()
            .find(|peer| peer.public_key_hex == public_key_hex)
        {
            peer.machine_name = machine_name.to_string();
            self.save()?;
            return Ok(TrustOutcome::Trusted);
        }

        Ok(TrustOutcome::Untrusted {
            machine_name: machine_name.to_string(),
        })
    }

    pub fn trust_peer(&mut self, machine_name: &str, public_key_hex: &str) -> Result<()> {
        let machine_name = machine_name.trim();
        let public_key_hex = public_key_hex.trim().to_lowercase();
        let _ = public_key_from_hex(&public_key_hex)?;
        self.data
            .trusted_peers
            .retain(|peer| peer.machine_name != machine_name && peer.public_key_hex != public_key_hex);
        self.data.trusted_peers.push(TrustedPeer {
            machine_name: machine_name.to_string(),
            public_key_hex,
            trusted_at_unix_secs: now_unix_secs(),
        });
        self.data
            .trusted_peers
            .sort_by(|left, right| left.machine_name.cmp(&right.machine_name));
        self.save()
    }

    pub fn trust_validity_days(&self) -> u32 {
        self.data.trust_validity_days.max(1)
    }

    pub fn set_trust_validity_days(&mut self, days: u32) -> Result<()> {
        self.data.trust_validity_days = days.max(1);
        self.prune_expired_peers();
        self.save()
    }

    pub fn trusted_peer_views(&mut self) -> Result<Vec<TrustedPeerView>> {
        self.prune_expired_peers();
        let validity_days = self.trust_validity_days();
        let now = now_unix_secs();
        let mut peers = self
            .data
            .trusted_peers
            .iter()
            .map(|peer| TrustedPeerView {
                machine_name: peer.machine_name.clone(),
                fingerprint: public_key_fingerprint(&peer.public_key_hex).unwrap_or_else(|_| "Invalid fingerprint".to_string()),
                expires_in_days: expiry_remaining_days(peer.trusted_at_unix_secs, validity_days, now),
            })
            .collect::<Vec<_>>();
        peers.sort_by(|left, right| left.machine_name.cmp(&right.machine_name));
        Ok(peers)
    }

    pub fn remove_trusted_peer_by_fingerprint(&mut self, fingerprint: &str) -> Result<bool> {
        let normalized = fingerprint.trim().to_uppercase();
        let before = self.data.trusted_peers.len();
        self.data.trusted_peers.retain(|peer| {
            public_key_fingerprint(&peer.public_key_hex)
                .map(|value| value != normalized)
                .unwrap_or(true)
        });
        let removed = self.data.trusted_peers.len() != before;
        if removed {
            self.save()?;
        }
        Ok(removed)
    }

    pub fn clear_trusted_peers(&mut self) -> Result<()> {
        self.data.trusted_peers.clear();
        self.save()
    }

    fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create security directory {}", parent.display())
            })?;
        }

        let bytes =
            serde_json::to_vec_pretty(&self.data).context("failed to serialize security file")?;
        fs::write(&self.path, bytes)
            .with_context(|| format!("failed to write security file {}", self.path.display()))
    }

    fn prune_expired_peers(&mut self) {
        let validity_days = self.trust_validity_days();
        let now = now_unix_secs();
        self.data
            .trusted_peers
            .retain(|peer| !is_expired(peer.trusted_at_unix_secs, validity_days, now));
    }
}

pub fn pairing_fingerprint(local_public_key_hex: &str, peer_public_key_hex: &str) -> Result<String> {
    let mut parts = [
        hex::decode(local_public_key_hex.trim()).context("invalid local public key encoding")?,
        hex::decode(peer_public_key_hex.trim()).context("invalid peer public key encoding")?,
    ];
    parts.sort();
    let combined = [parts[0].as_slice(), parts[1].as_slice()].concat();
    let digest = Sha256::digest(combined);
    Ok(format_fingerprint(&digest[..16]))
}

pub fn public_key_fingerprint(public_key_hex: &str) -> Result<String> {
    let bytes = hex::decode(public_key_hex.trim()).context("invalid public key encoding")?;
    let digest = Sha256::digest(bytes);
    Ok(format_fingerprint(&digest[..16]))
}

fn format_fingerprint(bytes: &[u8]) -> String {
    bytes
        .chunks(2)
        .map(hex::encode)
        .collect::<Vec<_>>()
        .join(":")
        .to_uppercase()
}

impl LocalIdentity {
    pub fn public_key_hex(&self) -> String {
        hex::encode(PublicKey::from(&self.secret).as_bytes())
    }

    pub fn derive_session_key(&self, peer_public_key_hex: &str) -> Result<[u8; 32]> {
        let peer_public_key = public_key_from_hex(peer_public_key_hex)?;
        let shared = self.secret.diffie_hellman(&peer_public_key);
        let hkdf = Hkdf::<Sha256>::new(None, shared.as_bytes());
        let mut key = [0_u8; 32];
        hkdf.expand(SECURITY_CONTEXT, &mut key)
            .map_err(|_| anyhow!("failed to derive session key"))?;
        Ok(key)
    }
}

fn public_key_from_hex(value: &str) -> Result<PublicKey> {
    let bytes = secret_bytes_from_hex(value)?;
    Ok(PublicKey::from(bytes))
}

fn secret_bytes_from_hex(value: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(value.trim()).context("invalid hex key encoding")?;
    if bytes.len() != 32 {
        bail!("expected a 32-byte key");
    }
    let mut array = [0_u8; 32];
    array.copy_from_slice(&bytes);
    Ok(array)
}

fn random_secret_bytes() -> Result<[u8; 32]> {
    let mut bytes = [0_u8; 32];
    random_fill(&mut bytes).map_err(|error| anyhow!("failed to generate local identity key: {error}"))?;
    Ok(bytes)
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
}

fn is_expired(trusted_at_unix_secs: u64, validity_days: u32, now_unix_secs: u64) -> bool {
    let validity_secs = validity_days as u64 * 24 * 60 * 60;
    now_unix_secs.saturating_sub(trusted_at_unix_secs) >= validity_secs
}

fn expiry_remaining_days(trusted_at_unix_secs: u64, validity_days: u32, now_unix_secs: u64) -> Option<u32> {
    if is_expired(trusted_at_unix_secs, validity_days, now_unix_secs) {
        return None;
    }
    let validity_secs = validity_days as u64 * 24 * 60 * 60;
    let expires_at = trusted_at_unix_secs.saturating_add(validity_secs);
    let remaining_secs = expires_at.saturating_sub(now_unix_secs);
    Some(((remaining_secs + 86_399) / 86_400) as u32)
}

fn security_path() -> Result<PathBuf> {
    if cfg!(target_os = "windows") {
        if let Ok(appdata) = env::var("APPDATA") {
            return Ok(PathBuf::from(appdata).join("velin").join("security.json"));
        }
    }

    if let Ok(xdg_config_home) = env::var("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(xdg_config_home).join("velin").join("security.json"));
    }

    let home = env::var("HOME")
        .or_else(|_| env::var("USERPROFILE"))
        .context("could not determine home directory for security settings")?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("velin")
        .join("security.json"))
}
