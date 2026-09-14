use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(test)]
use std::path::PathBuf;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use coding_tools_tunnel_protocol::{
    auth_signing_payload, valid_client_id, DeviceAuthProof, EnrollmentRequest, EnrollmentResponse,
    TunnelService,
};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::database::DatabaseWriter;

const DEFAULT_ENROLLMENT_TTL_SECONDS: u64 = 600;
const ENROLLMENT_RETENTION_MILLIS: u64 = 30 * 24 * 60 * 60 * 1000;
const MAX_DEVICE_NAME_BYTES: usize = 128;
const LAST_SEEN_FLUSH_INTERVAL_MILLIS: u64 = 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllowedServices {
    Mcp,
    Actions,
    Both,
}

impl AllowedServices {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "mcp" => Some(Self::Mcp),
            "actions" => Some(Self::Actions),
            "both" => Some(Self::Both),
            _ => None,
        }
    }

    fn flags(self) -> (i64, i64) {
        match self {
            Self::Mcp => (1, 0),
            Self::Actions => (0, 1),
            Self::Both => (1, 1),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mcp => "mcp",
            Self::Actions => "actions",
            Self::Both => "both",
        }
    }
}

#[derive(Debug, Clone)]
pub struct EnrollmentGrant {
    pub code: String,
    pub client_id: String,
    pub expires_at_unix_ms: u64,
    pub services: AllowedServices,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceSummary {
    pub device_id: String,
    pub client_id: String,
    pub device_name: String,
    pub allow_mcp: bool,
    pub allow_actions: bool,
    pub created_at_unix_ms: u64,
    pub last_seen_at_unix_ms: Option<u64>,
    pub revoked_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceAuthError {
    InvalidEnrollment,
    EnrollmentExpired,
    EnrollmentUsed,
    DeviceIdConflict,
    InvalidClientId,
    InvalidDeviceName,
    InvalidPublicKey,
    UnknownDevice,
    RevokedDevice,
    DeviceNotRevoked,
    ServiceNotAllowed,
    ChallengeExpired,
    InvalidSignature,
    Storage(String),
}

impl DeviceAuthError {
    pub fn public_message(&self) -> &'static str {
        match self {
            Self::InvalidEnrollment => "invalid enrollment code",
            Self::EnrollmentExpired => "enrollment code expired",
            Self::EnrollmentUsed => "enrollment code already used",
            Self::DeviceIdConflict => "tunnel device id belongs to another public key",
            Self::InvalidClientId => "invalid client id",
            Self::InvalidDeviceName => "invalid device name",
            Self::InvalidPublicKey => "invalid device public key",
            Self::UnknownDevice => "unknown tunnel device",
            Self::RevokedDevice => "tunnel device has been revoked",
            Self::DeviceNotRevoked => "active tunnel device cannot be permanently deleted",
            Self::ServiceNotAllowed => "device is not allowed to use this tunnel service",
            Self::ChallengeExpired => "authentication challenge expired",
            Self::InvalidSignature => "invalid device signature",
            Self::Storage(_) => "device registry unavailable",
        }
    }
}

impl fmt::Display for DeviceAuthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(message) => write!(formatter, "{message}"),
            _ => formatter.write_str(self.public_message()),
        }
    }
}

impl std::error::Error for DeviceAuthError {}

impl From<rusqlite::Error> for DeviceAuthError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

#[derive(Clone)]
struct CachedDevice {
    summary: DeviceSummary,
    public_key: [u8; 32],
    persisted_last_seen_at_unix_ms: Option<u64>,
}

#[derive(Clone)]
pub struct DeviceRegistry {
    database: DatabaseWriter,
    devices: Arc<RwLock<HashMap<String, CachedDevice>>>,
}

impl DeviceRegistry {
    #[cfg(test)]
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, DeviceAuthError> {
        let database = DatabaseWriter::open(path.into())
            .map_err(|error| DeviceAuthError::Storage(error.to_string()))?;
        Self::from_writer(database)
    }

    pub fn from_writer(database: DatabaseWriter) -> Result<Self, DeviceAuthError> {
        let devices = database
            .call(|connection| {
                initialize_schema(connection)?;
                load_devices(connection)
            })
            .map_err(|error| DeviceAuthError::Storage(error.to_string()))??;
        Ok(Self {
            database,
            devices: Arc::new(RwLock::new(devices)),
        })
    }

    #[cfg(test)]
    pub fn database_writer(&self) -> DatabaseWriter {
        self.database.clone()
    }

    pub fn create_enrollment(
        &self,
        client_id: &str,
        services: AllowedServices,
        ttl_seconds: Option<u64>,
    ) -> Result<EnrollmentGrant, DeviceAuthError> {
        if !valid_client_id(client_id) {
            return Err(DeviceAuthError::InvalidClientId);
        }
        let ttl_seconds = ttl_seconds
            .unwrap_or(DEFAULT_ENROLLMENT_TTL_SECONDS)
            .clamp(30, 86_400);
        let created_at = unix_ms();
        let expires_at = created_at.saturating_add(ttl_seconds.saturating_mul(1000));
        let code = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let code_hash = Sha256::digest(code.as_bytes()).to_vec();
        let client_id = client_id.to_string();
        let stored_client_id = client_id.clone();
        let (allow_mcp, allow_actions) = services.flags();
        self.database
            .call(move |connection| {
                purge_stale_enrollments(connection, created_at)?;
                connection.execute(
                    "INSERT INTO enrollment_codes
                     (code_hash, client_id, allow_mcp, allow_actions, created_at, expires_at,
                      used_at, enrolled_device_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL)",
                    params![
                        code_hash,
                        stored_client_id,
                        allow_mcp,
                        allow_actions,
                        created_at as i64,
                        expires_at as i64
                    ],
                )?;
                Ok::<_, DeviceAuthError>(())
            })
            .map_err(|error| DeviceAuthError::Storage(error.to_string()))??;
        Ok(EnrollmentGrant {
            code,
            client_id,
            expires_at_unix_ms: expires_at,
            services,
        })
    }

    pub async fn enroll(
        &self,
        code: String,
        request: EnrollmentRequest,
    ) -> Result<EnrollmentResponse, DeviceAuthError> {
        let registry = self.clone();
        tokio::task::spawn_blocking(move || registry.enroll_blocking(code, request))
            .await
            .map_err(|error| DeviceAuthError::Storage(error.to_string()))?
    }

    pub async fn verify(
        &self,
        nonce: String,
        expires_at_unix_ms: u64,
        proof: DeviceAuthProof,
    ) -> Result<(), DeviceAuthError> {
        self.verify_cached(&nonce, expires_at_unix_ms, &proof)
    }

    pub fn list_devices(&self) -> Result<Vec<DeviceSummary>, DeviceAuthError> {
        let devices = self
            .devices
            .read()
            .map_err(|_| DeviceAuthError::Storage("device cache lock poisoned".into()))?;
        let mut summaries = devices
            .values()
            .map(|device| device.summary.clone())
            .collect::<Vec<_>>();
        summaries.sort_by_key(|device| std::cmp::Reverse(device.created_at_unix_ms));
        Ok(summaries)
    }

    pub fn revoke_device(&self, device_id: &str) -> Result<bool, DeviceAuthError> {
        let device_id = device_id.to_string();
        let cached_device_id = device_id.clone();
        let devices = self.devices.clone();
        let revoked_at = unix_ms();
        self.database
            .call(move |connection| {
                let changed = connection.execute(
                    "UPDATE devices SET revoked_at = ?2
                     WHERE device_id = ?1 AND revoked_at IS NULL",
                    params![device_id, revoked_at as i64],
                )?;
                if changed == 1 {
                    if let Some(device) = devices
                        .write()
                        .map_err(|_| DeviceAuthError::Storage("device cache lock poisoned".into()))?
                        .get_mut(&cached_device_id)
                    {
                        device.summary.revoked_at_unix_ms = Some(revoked_at);
                    }
                }
                Ok::<_, DeviceAuthError>(changed == 1)
            })
            .map_err(|error| DeviceAuthError::Storage(error.to_string()))?
    }

    pub fn delete_revoked_device(&self, device_id: &str) -> Result<(), DeviceAuthError> {
        let device_id = device_id.to_string();
        let cached_device_id = device_id.clone();
        let devices = self.devices.clone();
        self.database
            .call(move |connection| {
                let revoked_at = connection
                    .query_row(
                        "SELECT revoked_at FROM devices WHERE device_id = ?1",
                        params![device_id],
                        |row| row.get::<_, Option<i64>>(0),
                    )
                    .optional()?
                    .ok_or(DeviceAuthError::UnknownDevice)?;
                if revoked_at.is_none() {
                    return Err(DeviceAuthError::DeviceNotRevoked);
                }
                let changed = connection.execute(
                    "DELETE FROM devices WHERE device_id = ?1 AND revoked_at IS NOT NULL",
                    params![device_id],
                )?;
                if changed != 1 {
                    return Err(DeviceAuthError::UnknownDevice);
                }
                devices
                    .write()
                    .map_err(|_| DeviceAuthError::Storage("device cache lock poisoned".into()))?
                    .remove(&cached_device_id);
                Ok::<_, DeviceAuthError>(())
            })
            .map_err(|error| DeviceAuthError::Storage(error.to_string()))?
    }

    pub fn purge_revoked_devices(&self) -> Result<usize, DeviceAuthError> {
        let devices = self.devices.clone();
        self.database
            .call(move |connection| {
                let revoked_ids = {
                    let mut statement = connection
                        .prepare("SELECT device_id FROM devices WHERE revoked_at IS NOT NULL")?;
                    let ids = statement
                        .query_map([], |row| row.get::<_, String>(0))?
                        .collect::<Result<Vec<_>, _>>()?;
                    ids
                };
                if revoked_ids.is_empty() {
                    return Ok::<_, DeviceAuthError>(0);
                }
                let changed =
                    connection.execute("DELETE FROM devices WHERE revoked_at IS NOT NULL", [])?;
                let mut cache = devices
                    .write()
                    .map_err(|_| DeviceAuthError::Storage("device cache lock poisoned".into()))?;
                for device_id in revoked_ids {
                    cache.remove(&device_id);
                }
                Ok::<_, DeviceAuthError>(changed)
            })
            .map_err(|error| DeviceAuthError::Storage(error.to_string()))?
    }

    fn enroll_blocking(
        &self,
        code: String,
        request: EnrollmentRequest,
    ) -> Result<EnrollmentResponse, DeviceAuthError> {
        if !valid_client_id(&request.device_id) {
            return Err(DeviceAuthError::UnknownDevice);
        }
        let device_name = request.device_name.trim().to_string();
        if device_name.is_empty() || device_name.len() > MAX_DEVICE_NAME_BYTES {
            return Err(DeviceAuthError::InvalidDeviceName);
        }
        let public_key = decode_public_key(&request.public_key)?;
        let code_hash = Sha256::digest(code.trim().as_bytes()).to_vec();
        let now = unix_ms();
        let devices = self.devices.clone();
        self.database
            .call(move |connection| {
                purge_stale_enrollments(connection, now)?;
                let transaction =
                    connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let enrollment = transaction
                    .query_row(
                        "SELECT client_id, allow_mcp, allow_actions, expires_at, used_at,
                                enrolled_device_id
                         FROM enrollment_codes WHERE code_hash = ?1",
                        params![code_hash],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, i64>(1)?,
                                row.get::<_, i64>(2)?,
                                row.get::<_, i64>(3)?,
                                row.get::<_, Option<i64>>(4)?,
                                row.get::<_, Option<String>>(5)?,
                            ))
                        },
                    )
                    .optional()?
                    .ok_or(DeviceAuthError::InvalidEnrollment)?;
                if enrollment.4.is_some() {
                    if enrollment.5.as_deref() == Some(request.device_id.as_str()) {
                        let existing = transaction
                            .query_row(
                                "SELECT client_id, public_key FROM devices WHERE device_id = ?1",
                                params![request.device_id],
                                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
                            )
                            .optional()?;
                        if existing.as_ref().is_some_and(|(client_id, key)| {
                            client_id == &enrollment.0 && key == &public_key
                        }) {
                            return Ok(EnrollmentResponse {
                                device_id: request.device_id.clone(),
                                client_id: enrollment.0.clone(),
                                server_id: String::new(),
                                server_public_key: String::new(),
                            });
                        }
                    }
                    return Err(DeviceAuthError::EnrollmentUsed);
                }
                if enrollment.3 < now as i64 {
                    return Err(DeviceAuthError::EnrollmentExpired);
                }

                let device_id = request.device_id.clone();
                // The enrollment grant is authoritative. request.client_id remains on the wire
                // for compatibility with older desktop clients and must not select the route.
                let client_id = enrollment.0.clone();
                let existing_public_key = transaction
                    .query_row(
                        "SELECT public_key FROM devices WHERE device_id = ?1",
                        params![&device_id],
                        |row| row.get::<_, Vec<u8>>(0),
                    )
                    .optional()?;
                match existing_public_key {
                    Some(existing) if existing.as_slice() != public_key.as_slice() => {
                        return Err(DeviceAuthError::DeviceIdConflict);
                    }
                    Some(_) => {
                        transaction.execute(
                            "UPDATE devices SET
                               client_id = ?2,
                               device_name = ?3,
                               allow_mcp = ?4,
                               allow_actions = ?5,
                               created_at = ?6,
                               last_seen_at = NULL,
                               revoked_at = NULL
                             WHERE device_id = ?1",
                            params![
                                &device_id,
                                &client_id,
                                &device_name,
                                enrollment.1,
                                enrollment.2,
                                now as i64
                            ],
                        )?;
                    }
                    None => {
                        transaction.execute(
                            "INSERT INTO devices
                             (device_id, client_id, device_name, public_key, allow_mcp,
                              allow_actions, created_at, last_seen_at, revoked_at)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL)",
                            params![
                                &device_id,
                                &client_id,
                                &device_name,
                                public_key.as_slice(),
                                enrollment.1,
                                enrollment.2,
                                now as i64
                            ],
                        )?;
                    }
                }
                let consumed = transaction.execute(
                    "UPDATE enrollment_codes SET used_at = ?2, enrolled_device_id = ?3
                     WHERE code_hash = ?1 AND used_at IS NULL",
                    params![code_hash, now as i64, &device_id],
                )?;
                if consumed != 1 {
                    return Err(DeviceAuthError::EnrollmentUsed);
                }
                transaction.commit()?;

                let summary = DeviceSummary {
                    device_id: device_id.clone(),
                    client_id: client_id.clone(),
                    device_name,
                    allow_mcp: enrollment.1 != 0,
                    allow_actions: enrollment.2 != 0,
                    created_at_unix_ms: now,
                    last_seen_at_unix_ms: None,
                    revoked_at_unix_ms: None,
                };
                devices
                    .write()
                    .map_err(|_| DeviceAuthError::Storage("device cache lock poisoned".into()))?
                    .insert(
                        device_id.clone(),
                        CachedDevice {
                            summary,
                            public_key,
                            persisted_last_seen_at_unix_ms: None,
                        },
                    );
                Ok(EnrollmentResponse {
                    device_id,
                    client_id,
                    server_id: String::new(),
                    server_public_key: String::new(),
                })
            })
            .map_err(|error| DeviceAuthError::Storage(error.to_string()))?
    }

    fn verify_cached(
        &self,
        nonce: &str,
        expires_at_unix_ms: u64,
        proof: &DeviceAuthProof,
    ) -> Result<(), DeviceAuthError> {
        if unix_ms() > expires_at_unix_ms {
            return Err(DeviceAuthError::ChallengeExpired);
        }
        if !valid_client_id(&proof.hello.client_id) {
            return Err(DeviceAuthError::InvalidClientId);
        }
        let device = self
            .devices
            .read()
            .map_err(|_| DeviceAuthError::Storage("device cache lock poisoned".into()))?
            .get(&proof.device_id)
            .filter(|device| device.summary.client_id == proof.hello.client_id)
            .cloned()
            .ok_or(DeviceAuthError::UnknownDevice)?;
        if device.summary.revoked_at_unix_ms.is_some() {
            return Err(DeviceAuthError::RevokedDevice);
        }
        let service_allowed = match proof.hello.service {
            TunnelService::Mcp => device.summary.allow_mcp,
            TunnelService::Actions => device.summary.allow_actions,
        };
        if !service_allowed {
            return Err(DeviceAuthError::ServiceNotAllowed);
        }
        let verifying_key = VerifyingKey::from_bytes(&device.public_key)
            .map_err(|_| DeviceAuthError::InvalidPublicKey)?;
        let signature_bytes = URL_SAFE_NO_PAD
            .decode(proof.signature.as_bytes())
            .map_err(|_| DeviceAuthError::InvalidSignature)?;
        let signature = Signature::from_slice(&signature_bytes)
            .map_err(|_| DeviceAuthError::InvalidSignature)?;
        verifying_key
            .verify(&auth_signing_payload(nonce, proof), &signature)
            .map_err(|_| DeviceAuthError::InvalidSignature)?;
        self.record_last_seen(&proof.device_id, unix_ms())
    }

    fn record_last_seen(&self, device_id: &str, now: u64) -> Result<(), DeviceAuthError> {
        let should_flush = {
            let mut devices = self
                .devices
                .write()
                .map_err(|_| DeviceAuthError::Storage("device cache lock poisoned".into()))?;
            let device = devices
                .get_mut(device_id)
                .ok_or(DeviceAuthError::UnknownDevice)?;
            if device.summary.revoked_at_unix_ms.is_some() {
                return Err(DeviceAuthError::RevokedDevice);
            }
            device.summary.last_seen_at_unix_ms = Some(now);
            let should_flush = device
                .persisted_last_seen_at_unix_ms
                .is_none_or(|last_seen| {
                    now.saturating_sub(last_seen) >= LAST_SEEN_FLUSH_INTERVAL_MILLIS
                });
            if should_flush {
                device.persisted_last_seen_at_unix_ms = Some(now);
            }
            should_flush
        };
        if should_flush {
            let device_id = device_id.to_string();
            if let Err(error) = self.database.enqueue(move |connection| {
                connection
                    .execute(
                        "UPDATE devices SET last_seen_at = ?2 WHERE device_id = ?1",
                        params![device_id, now as i64],
                    )
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            }) {
                tracing::warn!(%error, "could not enqueue device last-seen update");
            }
        }
        Ok(())
    }

    #[cfg(test)]
    fn with_connection<T>(
        &self,
        operation: impl FnOnce(&Connection) -> Result<T, DeviceAuthError> + Send + 'static,
    ) -> Result<T, DeviceAuthError>
    where
        T: Send + 'static,
    {
        self.database
            .call(move |connection| operation(connection))
            .map_err(|error| DeviceAuthError::Storage(error.to_string()))?
    }
}

fn initialize_schema(connection: &Connection) -> Result<(), DeviceAuthError> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS enrollment_codes (
            code_hash BLOB PRIMARY KEY,
            client_id TEXT NOT NULL,
            allow_mcp INTEGER NOT NULL,
            allow_actions INTEGER NOT NULL,
            created_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL,
            used_at INTEGER,
            enrolled_device_id TEXT
         );
         CREATE TABLE IF NOT EXISTS devices (
            device_id TEXT PRIMARY KEY,
            client_id TEXT NOT NULL,
            device_name TEXT NOT NULL,
            public_key BLOB NOT NULL,
            allow_mcp INTEGER NOT NULL,
            allow_actions INTEGER NOT NULL,
            created_at INTEGER NOT NULL,
            last_seen_at INTEGER,
            revoked_at INTEGER
         );
         CREATE INDEX IF NOT EXISTS devices_client_id_idx ON devices(client_id);
         CREATE INDEX IF NOT EXISTS enrollment_expiry_idx ON enrollment_codes(expires_at);",
    )?;
    Ok(())
}

fn load_devices(connection: &Connection) -> Result<HashMap<String, CachedDevice>, DeviceAuthError> {
    let mut statement = connection.prepare(
        "SELECT device_id, client_id, device_name, public_key, allow_mcp, allow_actions,
                created_at, last_seen_at, revoked_at
         FROM devices",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Vec<u8>>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, i64>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, Option<i64>>(7)?,
            row.get::<_, Option<i64>>(8)?,
        ))
    })?;
    let mut devices = HashMap::new();
    for row in rows {
        let (
            device_id,
            client_id,
            device_name,
            public_key,
            allow_mcp,
            allow_actions,
            created_at,
            last_seen_at,
            revoked_at,
        ) = row?;
        let public_key: [u8; 32] = public_key.try_into().map_err(|_| {
            DeviceAuthError::Storage(format!(
                "device {device_id} has an invalid persisted public key"
            ))
        })?;
        let summary = DeviceSummary {
            device_id: device_id.clone(),
            client_id,
            device_name,
            allow_mcp: allow_mcp != 0,
            allow_actions: allow_actions != 0,
            created_at_unix_ms: created_at as u64,
            last_seen_at_unix_ms: last_seen_at.map(|value| value as u64),
            revoked_at_unix_ms: revoked_at.map(|value| value as u64),
        };
        devices.insert(
            device_id,
            CachedDevice {
                persisted_last_seen_at_unix_ms: summary.last_seen_at_unix_ms,
                summary,
                public_key,
            },
        );
    }
    Ok(devices)
}

fn purge_stale_enrollments(connection: &Connection, now: u64) -> Result<usize, DeviceAuthError> {
    let cutoff = now.saturating_sub(ENROLLMENT_RETENTION_MILLIS);
    Ok(connection.execute(
        "DELETE FROM enrollment_codes WHERE expires_at < ?1",
        params![cutoff as i64],
    )?)
}

fn decode_public_key(value: &str) -> Result<[u8; 32], DeviceAuthError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(value.trim().as_bytes())
        .map_err(|_| DeviceAuthError::InvalidPublicKey)?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| DeviceAuthError::InvalidPublicKey)?;
    VerifyingKey::from_bytes(&bytes).map_err(|_| DeviceAuthError::InvalidPublicKey)?;
    Ok(bytes)
}

pub fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn registry() -> (tempfile::TempDir, DeviceRegistry) {
        let directory = tempfile::tempdir().expect("tempdir");
        let registry = DeviceRegistry::open(directory.path().join("devices.db")).expect("registry");
        (directory, registry)
    }

    fn enrollment_exists(registry: &DeviceRegistry, code: &str) -> bool {
        let code_hash = Sha256::digest(code.as_bytes());
        registry
            .with_connection(move |connection| {
                let count = connection.query_row(
                    "SELECT COUNT(*) FROM enrollment_codes WHERE code_hash = ?1",
                    params![code_hash.as_slice()],
                    |row| row.get::<_, i64>(0),
                )?;
                Ok(count == 1)
            })
            .expect("enrollment count")
    }

    async fn enroll_test_device(
        registry: &DeviceRegistry,
        device_id: &str,
        client_id: &str,
        key_seed: u8,
    ) {
        let grant = registry
            .create_enrollment(client_id, AllowedServices::Both, Some(60))
            .expect("create enrollment");
        let signing_key = SigningKey::from_bytes(&[key_seed; 32]);
        registry
            .enroll(
                grant.code,
                EnrollmentRequest {
                    device_id: device_id.into(),
                    client_id: client_id.into(),
                    device_name: format!("{client_id} test device"),
                    public_key: URL_SAFE_NO_PAD.encode(signing_key.verifying_key().to_bytes()),
                },
            )
            .await
            .expect("enroll test device");
    }

    #[tokio::test]
    async fn permanently_deletes_only_revoked_devices() {
        let (_directory, registry) = registry();
        enroll_test_device(&registry, "cleanup-active", "cleanup-active-client", 21).await;
        enroll_test_device(&registry, "cleanup-one", "cleanup-client-one", 22).await;
        enroll_test_device(&registry, "cleanup-two", "cleanup-client-two", 23).await;

        assert_eq!(
            registry.delete_revoked_device("cleanup-active"),
            Err(DeviceAuthError::DeviceNotRevoked)
        );
        assert!(registry.revoke_device("cleanup-one").expect("revoke first"));
        assert!(registry
            .revoke_device("cleanup-two")
            .expect("revoke second"));

        registry
            .delete_revoked_device("cleanup-one")
            .expect("delete first revoked device");
        let after_single = registry
            .list_devices()
            .expect("devices after single cleanup");
        assert!(after_single
            .iter()
            .all(|device| device.device_id != "cleanup-one"));
        assert!(after_single
            .iter()
            .any(|device| device.device_id == "cleanup-active"));

        assert_eq!(registry.purge_revoked_devices().expect("purge revoked"), 1);
        let remaining = registry.list_devices().expect("remaining devices");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].device_id, "cleanup-active");
        assert_eq!(registry.purge_revoked_devices().expect("empty purge"), 0);
        assert_eq!(
            registry.delete_revoked_device("missing-device"),
            Err(DeviceAuthError::UnknownDevice)
        );
    }

    #[tokio::test]
    async fn enrollment_access_lazily_purges_links_expired_over_thirty_days() {
        let (_directory, registry) = registry();
        let stale = registry
            .create_enrollment("stale-client", AllowedServices::Mcp, Some(60))
            .expect("stale grant");
        let recent = registry
            .create_enrollment("recent-client", AllowedServices::Mcp, Some(60))
            .expect("recent grant");
        let now = unix_ms();
        let cutoff = now.saturating_sub(ENROLLMENT_RETENTION_MILLIS);
        let stale_code = stale.code.clone();
        let recent_code = recent.code.clone();
        registry
            .with_connection(move |connection| {
                let stale_hash = Sha256::digest(stale_code.as_bytes());
                connection.execute(
                    "UPDATE enrollment_codes SET expires_at = ?2 WHERE code_hash = ?1",
                    params![stale_hash.as_slice(), cutoff.saturating_sub(60_000) as i64],
                )?;
                let recent_hash = Sha256::digest(recent_code.as_bytes());
                connection.execute(
                    "UPDATE enrollment_codes SET expires_at = ?2 WHERE code_hash = ?1",
                    params![recent_hash.as_slice(), cutoff.saturating_add(60_000) as i64],
                )?;
                Ok(())
            })
            .expect("set enrollment expiry");

        assert!(enrollment_exists(&registry, &stale.code));
        assert!(enrollment_exists(&registry, &recent.code));

        let signing_key = SigningKey::from_bytes(&[5_u8; 32]);
        let result = registry
            .enroll(
                "missing-enrollment-code".into(),
                EnrollmentRequest {
                    device_id: "cleanup-device".into(),
                    client_id: "cleanup-client".into(),
                    device_name: "cleanup test".into(),
                    public_key: URL_SAFE_NO_PAD.encode(signing_key.verifying_key().to_bytes()),
                },
            )
            .await;

        assert_eq!(result, Err(DeviceAuthError::InvalidEnrollment));
        assert!(!enrollment_exists(&registry, &stale.code));
        assert!(enrollment_exists(&registry, &recent.code));
    }

    #[tokio::test]
    async fn enrollment_is_one_time_and_signature_is_verified() {
        let (_directory, registry) = registry();
        let grant = registry
            .create_enrollment("pc-a", AllowedServices::Both, Some(60))
            .expect("grant");
        let signing_key = SigningKey::from_bytes(&[7_u8; 32]);
        let request = EnrollmentRequest {
            device_id: "device-1".into(),
            client_id: "client-side-placeholder".into(),
            device_name: "test device".into(),
            public_key: URL_SAFE_NO_PAD.encode(signing_key.verifying_key().to_bytes()),
        };
        let enrolled = registry
            .enroll(grant.code.clone(), request.clone())
            .await
            .expect("enroll");
        assert_eq!(enrolled.client_id, "pc-a");
        assert_eq!(
            registry
                .enroll(grant.code.clone(), request.clone())
                .await
                .expect("idempotent enrollment retry"),
            enrolled
        );
        let mut different_device = request;
        different_device.device_id = "device-2".into();
        assert_eq!(
            registry.enroll(grant.code, different_device).await,
            Err(DeviceAuthError::EnrollmentUsed)
        );

        let nonce = "nonce-1";
        let mut proof = DeviceAuthProof {
            hello: coding_tools_tunnel_protocol::ClientHello {
                protocol_version: coding_tools_tunnel_protocol::PROTOCOL_VERSION,
                client_id: "pc-a".into(),
                service: TunnelService::Mcp,
                worker_id: "worker-1".into(),
            },
            device_id: enrolled.device_id,
            signature: String::new(),
        };
        proof.signature = URL_SAFE_NO_PAD.encode(
            signing_key
                .sign(&auth_signing_payload(nonce, &proof))
                .to_bytes(),
        );
        registry
            .verify(nonce.into(), unix_ms() + 5_000, proof)
            .await
            .expect("verify");
    }

    #[tokio::test]
    async fn revoked_device_cannot_authenticate() {
        let (_directory, registry) = registry();
        let grant = registry
            .create_enrollment("pc-a", AllowedServices::Mcp, Some(60))
            .expect("grant");
        let signing_key = SigningKey::from_bytes(&[9_u8; 32]);
        let enrolled = registry
            .enroll(
                grant.code,
                EnrollmentRequest {
                    device_id: "device-2".into(),
                    client_id: "pc-a".into(),
                    device_name: "revoked device".into(),
                    public_key: URL_SAFE_NO_PAD.encode(signing_key.verifying_key().to_bytes()),
                },
            )
            .await
            .expect("enroll");
        assert!(registry.revoke_device(&enrolled.device_id).expect("revoke"));
        let mut proof = DeviceAuthProof {
            hello: coding_tools_tunnel_protocol::ClientHello {
                protocol_version: coding_tools_tunnel_protocol::PROTOCOL_VERSION,
                client_id: "pc-a".into(),
                service: TunnelService::Mcp,
                worker_id: "worker-1".into(),
            },
            device_id: enrolled.device_id,
            signature: String::new(),
        };
        proof.signature = URL_SAFE_NO_PAD.encode(
            signing_key
                .sign(&auth_signing_payload("nonce", &proof))
                .to_bytes(),
        );
        assert_eq!(
            registry
                .verify("nonce".into(), unix_ms() + 5_000, proof)
                .await,
            Err(DeviceAuthError::RevokedDevice)
        );
    }

    #[tokio::test]
    async fn new_grant_reactivates_the_same_device_key_but_rejects_a_different_key() {
        let (_directory, registry) = registry();
        let signing_key = SigningKey::from_bytes(&[13_u8; 32]);
        let request = EnrollmentRequest {
            device_id: "recoverable-device".into(),
            client_id: "ignored-placeholder".into(),
            device_name: "recoverable device".into(),
            public_key: URL_SAFE_NO_PAD.encode(signing_key.verifying_key().to_bytes()),
        };
        let first = registry
            .create_enrollment("old-client", AllowedServices::Mcp, Some(60))
            .expect("first grant");
        registry
            .enroll(first.code, request.clone())
            .await
            .expect("first enrollment");
        assert!(registry
            .revoke_device(&request.device_id)
            .expect("revoke first enrollment"));

        let recovery = registry
            .create_enrollment("new-client", AllowedServices::Actions, Some(60))
            .expect("recovery grant");
        let recovered = registry
            .enroll(recovery.code, request.clone())
            .await
            .expect("recovery enrollment");
        assert_eq!(recovered.client_id, "new-client");
        let devices = registry.list_devices().expect("devices");
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].client_id, "new-client");
        assert!(!devices[0].allow_mcp);
        assert!(devices[0].allow_actions);
        assert_eq!(devices[0].revoked_at_unix_ms, None);

        let conflicting_key = SigningKey::from_bytes(&[14_u8; 32]);
        let conflict = registry
            .create_enrollment("hijack-client", AllowedServices::Both, Some(60))
            .expect("conflicting grant");
        let mut conflicting_request = request;
        conflicting_request.public_key =
            URL_SAFE_NO_PAD.encode(conflicting_key.verifying_key().to_bytes());
        assert_eq!(
            registry.enroll(conflict.code, conflicting_request).await,
            Err(DeviceAuthError::DeviceIdConflict)
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_enrollment_and_cached_authentication_share_one_writer() {
        let (_directory, registry) = registry();
        let mut workers = Vec::new();
        for index in 1..=24_u8 {
            let grant = registry
                .create_enrollment("shared-client", AllowedServices::Both, Some(60))
                .expect("grant");
            let registry = registry.clone();
            workers.push(tokio::spawn(async move {
                let signing_key = SigningKey::from_bytes(&[index; 32]);
                let device_id = format!("device-{index}");
                registry
                    .enroll(
                        grant.code,
                        EnrollmentRequest {
                            device_id: device_id.clone(),
                            client_id: "ignored-placeholder".into(),
                            device_name: format!("worker {index}"),
                            public_key: URL_SAFE_NO_PAD
                                .encode(signing_key.verifying_key().to_bytes()),
                        },
                    )
                    .await
                    .expect("concurrent enrollment");

                for attempt in 0..20_u8 {
                    let nonce = format!("nonce-{index}-{attempt}");
                    let mut proof = DeviceAuthProof {
                        hello: coding_tools_tunnel_protocol::ClientHello {
                            protocol_version: coding_tools_tunnel_protocol::PROTOCOL_VERSION,
                            client_id: "shared-client".into(),
                            service: TunnelService::Mcp,
                            worker_id: format!("worker-{index}"),
                        },
                        device_id: device_id.clone(),
                        signature: String::new(),
                    };
                    proof.signature = URL_SAFE_NO_PAD.encode(
                        signing_key
                            .sign(&auth_signing_payload(&nonce, &proof))
                            .to_bytes(),
                    );
                    registry
                        .verify(nonce, unix_ms() + 5_000, proof)
                        .await
                        .expect("cached verification");
                }
            }));
        }
        for worker in workers {
            worker.await.expect("worker task");
        }
        assert_eq!(registry.list_devices().expect("devices").len(), 24);
    }

    #[tokio::test]
    async fn last_seen_is_live_in_memory_and_throttled_on_disk() {
        let (_directory, registry) = registry();
        let grant = registry
            .create_enrollment("pc-a", AllowedServices::Mcp, Some(60))
            .expect("grant");
        let signing_key = SigningKey::from_bytes(&[11_u8; 32]);
        let enrolled = registry
            .enroll(
                grant.code,
                EnrollmentRequest {
                    device_id: "device-last-seen".into(),
                    client_id: "pc-a".into(),
                    device_name: "last seen test".into(),
                    public_key: URL_SAFE_NO_PAD.encode(signing_key.verifying_key().to_bytes()),
                },
            )
            .await
            .expect("enroll");

        let signed_proof = |nonce: &str| {
            let mut proof = DeviceAuthProof {
                hello: coding_tools_tunnel_protocol::ClientHello {
                    protocol_version: coding_tools_tunnel_protocol::PROTOCOL_VERSION,
                    client_id: "pc-a".into(),
                    service: TunnelService::Mcp,
                    worker_id: "worker-last-seen".into(),
                },
                device_id: enrolled.device_id.clone(),
                signature: String::new(),
            };
            proof.signature = URL_SAFE_NO_PAD.encode(
                signing_key
                    .sign(&auth_signing_payload(nonce, &proof))
                    .to_bytes(),
            );
            proof
        };

        registry
            .verify("first".into(), unix_ms() + 5_000, signed_proof("first"))
            .await
            .expect("first verification");
        let first_id = enrolled.device_id.clone();
        let first_persisted = registry
            .with_connection(move |connection| {
                Ok(connection.query_row(
                    "SELECT last_seen_at FROM devices WHERE device_id = ?1",
                    params![first_id],
                    |row| row.get::<_, Option<i64>>(0),
                )?)
            })
            .expect("first persisted timestamp")
            .expect("first last seen") as u64;

        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        registry
            .verify("second".into(), unix_ms() + 5_000, signed_proof("second"))
            .await
            .expect("second verification");
        let second_id = enrolled.device_id.clone();
        let second_persisted = registry
            .with_connection(move |connection| {
                Ok(connection.query_row(
                    "SELECT last_seen_at FROM devices WHERE device_id = ?1",
                    params![second_id],
                    |row| row.get::<_, Option<i64>>(0),
                )?)
            })
            .expect("second persisted timestamp")
            .expect("second last seen") as u64;
        let memory_last_seen = registry
            .list_devices()
            .expect("devices")
            .into_iter()
            .find(|device| device.device_id == enrolled.device_id)
            .and_then(|device| device.last_seen_at_unix_ms)
            .expect("memory last seen");

        assert_eq!(second_persisted, first_persisted);
        assert!(memory_last_seen > first_persisted);
    }
}
