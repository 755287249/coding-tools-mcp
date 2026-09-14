use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use coding_tools_tunnel_protocol::{
    auth_signing_payload, server_ack_signing_payload, server_challenge_signing_payload,
    ClientHello, ControlMessage, DeviceAuthProof, WorkerPolicy, CLIENT_ID_HEADER, DEVICE_ID_HEADER,
    PROTOCOL_VERSION, SERVICE_HEADER, WORKER_ID_HEADER, WS_SUBPROTOCOL,
};
use ed25519_dalek::{Signature, Signer, Verifier};
use futures_util::StreamExt;
use tokio::time::timeout;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::SEC_WEBSOCKET_PROTOCOL;

use super::protocol_io::{receive_control, send_control, ClientSink, ClientStream};
use super::{BuiltinTunnelConfig, WEBSOCKET_CONNECT_TIMEOUT};

pub(super) struct AuthenticatedWorkerConnection {
    pub(super) sink: ClientSink,
    pub(super) stream: ClientStream,
    pub(super) initial_policy: WorkerPolicy,
}

pub(super) async fn connect_authenticated_worker(
    config: &BuiltinTunnelConfig,
    worker_id: &str,
) -> Result<AuthenticatedWorkerConnection, String> {
    let mut request = config
        .websocket_url
        .clone()
        .into_client_request()
        .map_err(|error| error.to_string())?;
    request.headers_mut().insert(
        CLIENT_ID_HEADER,
        config
            .client_id
            .parse()
            .map_err(|error| format!("invalid client id header: {error}"))?,
    );
    request.headers_mut().insert(
        SERVICE_HEADER,
        config
            .service
            .as_str()
            .parse()
            .map_err(|error| format!("invalid service header: {error}"))?,
    );
    request.headers_mut().insert(
        DEVICE_ID_HEADER,
        config
            .device_id
            .parse()
            .map_err(|error| format!("invalid device id header: {error}"))?,
    );
    request.headers_mut().insert(
        WORKER_ID_HEADER,
        worker_id
            .parse()
            .map_err(|error| format!("invalid worker id header: {error}"))?,
    );
    request.headers_mut().insert(
        SEC_WEBSOCKET_PROTOCOL,
        WS_SUBPROTOCOL
            .parse()
            .map_err(|error| format!("invalid tunnel subprotocol: {error}"))?,
    );

    let (socket, response) = timeout(WEBSOCKET_CONNECT_TIMEOUT, connect_async(request))
        .await
        .map_err(|_| "WSS connection timed out".to_string())?
        .map_err(|error| error.to_string())?;
    if response
        .headers()
        .get(SEC_WEBSOCKET_PROTOCOL)
        .and_then(|value| value.to_str().ok())
        != Some(WS_SUBPROTOCOL)
    {
        return Err(format!("server did not accept {WS_SUBPROTOCOL}"));
    }

    let (mut sink, mut stream) = socket.split();
    let (nonce, expires_at_unix_ms, server_id, server_signature) =
        match receive_control(&mut sink, &mut stream).await? {
            ControlMessage::Challenge {
                nonce,
                expires_at_unix_ms,
                server_id,
                server_signature,
            } => (nonce, expires_at_unix_ms, server_id, server_signature),
            ControlMessage::Error { message, .. } => return Err(message),
            _ => return Err("server did not issue a device authentication challenge".into()),
        };
    if unix_ms() > expires_at_unix_ms {
        return Err("server authentication challenge already expired".into());
    }
    if server_id != config.server_id {
        return Err("tunnel server identity does not match the enrolled server".into());
    }
    verify_server_signature(
        &config.server_verifying_key,
        &server_challenge_signing_payload(
            &nonce,
            expires_at_unix_ms,
            &server_id,
            &config.device_id,
            &config.client_id,
            config.service,
            worker_id,
        ),
        &server_signature,
        "challenge",
    )?;

    let mut proof = DeviceAuthProof {
        hello: ClientHello {
            protocol_version: PROTOCOL_VERSION,
            client_id: config.client_id.clone(),
            service: config.service,
            worker_id: worker_id.to_string(),
        },
        device_id: config.device_id.clone(),
        signature: String::new(),
    };
    proof.signature = URL_SAFE_NO_PAD.encode(
        config
            .signing_key
            .sign(&auth_signing_payload(&nonce, &proof))
            .to_bytes(),
    );
    send_control(&mut sink, &ControlMessage::Authenticate(proof.clone())).await?;

    let initial_policy = match receive_control(&mut sink, &mut stream).await? {
        ControlMessage::HelloAck {
            protocol_version,
            worker_policy,
            server_id,
            server_signature,
        } if protocol_version == PROTOCOL_VERSION => {
            if server_id != config.server_id {
                return Err(
                    "tunnel server acknowledgement identity changed during authentication".into(),
                );
            }
            verify_server_signature(
                &config.server_verifying_key,
                &server_ack_signing_payload(&nonce, &server_id, &proof, &worker_policy),
                &server_signature,
                "acknowledgement",
            )?;
            worker_policy
        }
        ControlMessage::Error { message, .. } => return Err(message),
        _ => return Err("server did not acknowledge tunnel device authentication".into()),
    };
    initial_policy.validate()?;

    Ok(AuthenticatedWorkerConnection {
        sink,
        stream,
        initial_policy,
    })
}

fn verify_server_signature(
    key: &ed25519_dalek::VerifyingKey,
    payload: &[u8],
    encoded_signature: &str,
    context: &str,
) -> Result<(), String> {
    let signature = URL_SAFE_NO_PAD
        .decode(encoded_signature.as_bytes())
        .map_err(|_| format!("tunnel server {context} signature is invalid"))?;
    let signature: [u8; 64] = signature
        .try_into()
        .map_err(|_| format!("tunnel server {context} signature has invalid length"))?;
    key.verify(payload, &Signature::from_bytes(&signature))
        .map_err(|_| format!("tunnel server {context} signature is invalid"))
}

pub(super) fn unix_ms() -> u64 {
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
    use ed25519_dalek::SigningKey;

    #[test]
    fn rejects_server_signature_from_a_different_key() {
        let trusted = SigningKey::from_bytes(&[41_u8; 32]);
        let attacker = SigningKey::from_bytes(&[42_u8; 32]);
        let payload = b"signed-server-challenge";
        let forged = URL_SAFE_NO_PAD.encode(attacker.sign(payload).to_bytes());

        let error =
            verify_server_signature(&trusted.verifying_key(), payload, &forged, "challenge")
                .expect_err("a different server key must not be accepted");
        assert!(error.contains("signature is invalid"));
    }

    #[test]
    fn accepts_server_signature_from_the_pinned_key() {
        let trusted = SigningKey::from_bytes(&[43_u8; 32]);
        let payload = b"signed-server-ack";
        let signature = URL_SAFE_NO_PAD.encode(trusted.sign(payload).to_bytes());

        verify_server_signature(
            &trusted.verifying_key(),
            payload,
            &signature,
            "acknowledgement",
        )
        .expect("the pinned server key should verify");
    }
}
