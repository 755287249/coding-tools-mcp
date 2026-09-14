use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use coding_tools_tunnel_protocol::{
    valid_client_id, EnrollmentRequest, EnrollmentResponse, ENROLL_PATH_PREFIX,
};
use ed25519_dalek::{SigningKey, VerifyingKey};
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::secret::SecretStore;

const DEVICE_IDENTITY_KEY: &str = "builtin_tunnel_device_identity";
const ENROLLMENT_URL_KEY: &str = "builtin_tunnel_enrollment_url";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct StoredDeviceIdentity {
    pub(super) device_id: String,
    pub(super) client_id: String,
    pub(super) private_key: String,
    enrolled: bool,
    #[serde(default)]
    pub(super) server_id: String,
    #[serde(default)]
    pub(super) server_public_key: String,
}

fn expected_server_id(public_key: &str) -> AppResult<String> {
    let key = decode_server_verifying_key(public_key)?;
    Ok(URL_SAFE_NO_PAD.encode(Sha256::digest(key.to_bytes())))
}

fn has_pinned_server_identity(identity: &StoredDeviceIdentity) -> bool {
    identity.server_id.len() == 43
        && identity
            .server_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        && expected_server_id(&identity.server_public_key)
            .is_ok_and(|expected| expected == identity.server_id)
}

pub(super) async fn load_or_enroll_device_identity(
    profile_id: &str,
    public_url: &str,
    client_id: &str,
) -> AppResult<StoredDeviceIdentity> {
    let stored = SecretStore::get(profile_id, DEVICE_IDENTITY_KEY)?
        .map(|raw| {
            serde_json::from_str::<StoredDeviceIdentity>(&raw)
                .map_err(|error| AppError::Message(format!("內建隧道裝置身分格式損壞：{error}")))
        })
        .transpose()?;
    let enrollment_url =
        SecretStore::get(profile_id, ENROLLMENT_URL_KEY)?.filter(|value| !value.trim().is_empty());

    if let Some(identity) = stored
        .as_ref()
        .filter(|identity| identity.enrolled && has_pinned_server_identity(identity))
    {
        return Ok(identity.clone());
    }

    let mut identity = match stored {
        Some(identity) => identity,
        None => {
            let signing_key = SigningKey::generate(&mut OsRng);
            let identity = StoredDeviceIdentity {
                device_id: Uuid::new_v4().simple().to_string(),
                client_id: client_id.to_string(),
                private_key: URL_SAFE_NO_PAD.encode(signing_key.to_bytes()),
                enrolled: false,
                server_id: String::new(),
                server_public_key: String::new(),
            };
            save_device_identity(profile_id, &identity)?;
            identity
        }
    };

    let enrollment_url = enrollment_url.ok_or_else(|| {
        if identity.enrolled {
            AppError::Message("內建 WSS 隧道 v4 需要重新註冊一次，以綁定伺服器身分金鑰。".into())
        } else {
            AppError::Message("內建 WSS 隧道尚未註冊。請貼上伺服器產生的一次性註冊連結。".into())
        }
    })?;
    let enrollment_url = parse_enrollment_url(public_url, &enrollment_url)?;
    let signing_key = decode_signing_key(&identity.private_key)?;
    let request = EnrollmentRequest {
        device_id: identity.device_id.clone(),
        client_id: identity.client_id.clone(),
        device_name: local_device_name(),
        public_key: URL_SAFE_NO_PAD.encode(signing_key.verifying_key().to_bytes()),
    };
    let response = reqwest::Client::builder()
        .connect_timeout(super::WEBSOCKET_CONNECT_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| AppError::Message(format!("無法建立裝置註冊連線：{error}")))?
        .post(enrollment_url)
        .json(&request)
        .send()
        .await
        .map_err(|error| AppError::Message(format!("裝置註冊連線失敗：{error}")))?;
    if !response.status().is_success() {
        let status = response.status();
        let detail = response.text().await.unwrap_or_default();
        return Err(AppError::Message(format!(
            "內建隧道裝置註冊失敗（{status}）：{}",
            detail.trim()
        )));
    }
    let enrolled = response
        .json::<EnrollmentResponse>()
        .await
        .map_err(|error| AppError::Message(format!("裝置註冊回應格式無效：{error}")))?;
    if enrolled.device_id != identity.device_id {
        return Err(AppError::Message(
            "內建隧道伺服器回傳了不一致的裝置 ID。".into(),
        ));
    }
    let enrolled_client_id = if enrolled.client_id.trim().is_empty() {
        identity.client_id.clone()
    } else {
        enrolled.client_id.trim().to_string()
    };
    if !valid_client_id(&enrolled_client_id) {
        return Err(AppError::Message(
            "內建隧道伺服器回傳了無效的 Client ID。".into(),
        ));
    }
    if enrolled.server_id.len() != 43
        || !enrolled
            .server_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        || expected_server_id(&enrolled.server_public_key)
            .map_or(true, |expected| expected != enrolled.server_id)
    {
        return Err(AppError::Message(
            "內建隧道伺服器回傳了無效的伺服器身分。".into(),
        ));
    }

    identity.client_id = enrolled_client_id;
    identity.enrolled = true;
    identity.server_id = enrolled.server_id;
    identity.server_public_key = enrolled.server_public_key;
    save_device_identity(profile_id, &identity)?;
    SecretStore::set(profile_id, ENROLLMENT_URL_KEY, "")?;
    Ok(identity)
}

fn save_device_identity(profile_id: &str, identity: &StoredDeviceIdentity) -> AppResult<()> {
    let encoded = serde_json::to_string(identity)?;
    SecretStore::set(profile_id, DEVICE_IDENTITY_KEY, &encoded)
}

pub(super) fn decode_signing_key(value: &str) -> AppResult<SigningKey> {
    let bytes = URL_SAFE_NO_PAD
        .decode(value.trim().as_bytes())
        .map_err(|_| AppError::Message("內建隧道裝置私鑰格式無效。".into()))?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| AppError::Message("內建隧道裝置私鑰長度無效。".into()))?;
    Ok(SigningKey::from_bytes(&bytes))
}

pub(super) fn decode_server_verifying_key(value: &str) -> AppResult<VerifyingKey> {
    let bytes = URL_SAFE_NO_PAD
        .decode(value.trim().as_bytes())
        .map_err(|_| AppError::Message("內建隧道伺服器公鑰格式無效。".into()))?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| AppError::Message("內建隧道伺服器公鑰長度無效。".into()))?;
    VerifyingKey::from_bytes(&bytes)
        .map_err(|_| AppError::Message("內建隧道伺服器公鑰無效。".into()))
}

pub(super) fn parse_enrollment_url(public_url: &str, value: &str) -> AppResult<reqwest::Url> {
    let public = reqwest::Url::parse(public_url)
        .map_err(|_| AppError::Message("內建隧道公開網址格式無效。".into()))?;
    let enrollment = reqwest::Url::parse(value.trim())
        .map_err(|_| AppError::Message("一次性註冊連結格式無效。".into()))?;
    if enrollment.scheme() != "https"
        || enrollment.username() != ""
        || enrollment.password().is_some()
        || enrollment.query().is_some()
        || enrollment.fragment().is_some()
        || enrollment.host_str() != public.host_str()
        || enrollment.port_or_known_default() != public.port_or_known_default()
    {
        return Err(AppError::Message(
            "一次性註冊連結必須使用與內建隧道相同的 HTTPS 網域與連接埠。".into(),
        ));
    }
    let prefix = format!("{ENROLL_PATH_PREFIX}/");
    let code = enrollment
        .path()
        .strip_prefix(&prefix)
        .filter(|code| {
            !code.is_empty()
                && code.len() <= 128
                && code.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
        .ok_or_else(|| {
            AppError::Message("一次性註冊連結必須使用 /_tunnel/enroll/<code> 路徑。".into())
        })?;
    if code.contains('/') {
        return Err(AppError::Message("一次性註冊碼格式無效。".into()));
    }
    Ok(enrollment)
}

fn local_device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Coding Tools MCP".into())
}
