use std::fs;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone)]
pub struct ServerIdentity {
    signing_key: SigningKey,
    server_id: String,
    public_key: String,
}

#[derive(Serialize, Deserialize)]
struct StoredServerIdentity {
    version: u8,
    private_key: String,
}

impl ServerIdentity {
    pub fn load_or_create(path: &Path) -> Result<Self, String> {
        if path.exists() {
            return Self::load(path);
        }
        let mut private_key = [0_u8; 32];
        OsRng.fill_bytes(&mut private_key);
        let identity = Self::from_signing_key(SigningKey::from_bytes(&private_key));
        identity.persist(path)?;
        Ok(identity)
    }

    fn load(path: &Path) -> Result<Self, String> {
        let stored: StoredServerIdentity = serde_json::from_slice(
            &fs::read(path).map_err(|error| format!("could not read server identity: {error}"))?,
        )
        .map_err(|error| format!("invalid server identity file: {error}"))?;
        if stored.version != 1 {
            return Err(format!(
                "unsupported server identity version {}",
                stored.version
            ));
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(stored.private_key.as_bytes())
            .map_err(|_| "invalid server identity private key".to_string())?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| "invalid server identity private key length".to_string())?;
        Ok(Self::from_signing_key(SigningKey::from_bytes(&bytes)))
    }

    fn from_signing_key(signing_key: SigningKey) -> Self {
        let public_bytes = signing_key.verifying_key().to_bytes();
        let public_key = URL_SAFE_NO_PAD.encode(public_bytes);
        let server_id = URL_SAFE_NO_PAD.encode(Sha256::digest(public_bytes));
        Self {
            signing_key,
            server_id,
            public_key,
        }
    }

    fn persist(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)
                .map_err(|error| format!("could not create server identity directory: {error}"))?;
        }
        let stored = StoredServerIdentity {
            version: 1,
            private_key: URL_SAFE_NO_PAD.encode(self.signing_key.to_bytes()),
        };
        let temporary = temporary_path(path);
        fs::write(
            &temporary,
            serde_json::to_vec_pretty(&stored)
                .map_err(|error| format!("could not encode server identity: {error}"))?,
        )
        .map_err(|error| format!("could not write server identity: {error}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))
                .map_err(|error| format!("could not protect server identity: {error}"))?;
        }
        fs::rename(&temporary, path)
            .map_err(|error| format!("could not install server identity: {error}"))?;
        Ok(())
    }

    pub fn server_id(&self) -> &str {
        &self.server_id
    }

    pub fn public_key(&self) -> &str {
        &self.public_key
    }

    pub fn sign(&self, payload: &[u8]) -> String {
        URL_SAFE_NO_PAD.encode(self.signing_key.sign(payload).to_bytes())
    }
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut temporary = path.as_os_str().to_os_string();
    temporary.push(format!(".{}.tmp", std::process::id()));
    PathBuf::from(temporary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

    #[test]
    fn identity_survives_restart_and_signatures_verify() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("server-identity.json");
        let first = ServerIdentity::load_or_create(&path).unwrap();
        let second = ServerIdentity::load_or_create(&path).unwrap();
        assert_eq!(first.server_id(), second.server_id());
        assert_eq!(first.public_key(), second.public_key());

        let public: [u8; 32] = URL_SAFE_NO_PAD
            .decode(first.public_key().as_bytes())
            .unwrap()
            .try_into()
            .unwrap();
        let signature: [u8; 64] = URL_SAFE_NO_PAD
            .decode(first.sign(b"challenge").as_bytes())
            .unwrap()
            .try_into()
            .unwrap();
        VerifyingKey::from_bytes(&public)
            .unwrap()
            .verify(b"challenge", &Signature::from_bytes(&signature))
            .unwrap();
    }
}
