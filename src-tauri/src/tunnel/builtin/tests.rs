use super::*;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use coding_tools_tunnel_protocol::{
    server_ack_signing_payload, server_challenge_signing_payload, ControlMessage, DEVICE_ID_HEADER,
    PROTOCOL_VERSION, WORKER_ID_HEADER, WS_PATH, WS_SUBPROTOCOL,
};
use ed25519_dalek::{Signer, VerifyingKey};
use std::sync::Mutex as StdMutex;
use tokio_tungstenite::tungstenite::http::header::SEC_WEBSOCKET_PROTOCOL;

use super::connection::unix_ms;
use super::identity::parse_enrollment_url;
use super::pool_policy::{
    PoolAdjustment, ScaleUpBlock, ESTABLISHED_RECONNECT_DELAY, MAX_RECONNECT_DELAY,
};
use super::request_mapping::local_path_for_request;

fn single_worker_policy() -> WorkerPolicy {
    let mut policy = WorkerPolicy::default_for(TunnelService::Mcp);
    policy.start_workers = 1;
    policy.min_idle_workers = 1;
    policy.max_idle_workers = 1;
    policy.max_workers = 1;
    policy
}

const TEST_SERVER_ID: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

fn test_server_signing_key() -> SigningKey {
    SigningKey::from_bytes(&[91_u8; 32])
}

fn test_server_verifying_key() -> Arc<VerifyingKey> {
    Arc::new(test_server_signing_key().verifying_key())
}

async fn accept_authenticated_test_worker(
    stream: tokio::net::TcpStream,
    nonce: String,
    policy: WorkerPolicy,
) -> tokio_tungstenite::WebSocketStream<tokio::net::TcpStream> {
    let connection_identity = Arc::new(StdMutex::new(None::<(String, String)>));
    let captured = connection_identity.clone();
    let mut socket = tokio_tungstenite::accept_hdr_async(
        stream,
        move |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
              mut response: tokio_tungstenite::tungstenite::handshake::server::Response| {
            let device_id = request
                .headers()
                .get(DEVICE_ID_HEADER)
                .and_then(|value| value.to_str().ok())
                .expect("device id header")
                .to_string();
            let worker_id = request
                .headers()
                .get(WORKER_ID_HEADER)
                .and_then(|value| value.to_str().ok())
                .expect("worker id header")
                .to_string();
            *captured.lock().expect("capture connection identity") = Some((device_id, worker_id));
            response.headers_mut().insert(
                SEC_WEBSOCKET_PROTOCOL,
                WS_SUBPROTOCOL.parse().expect("subprotocol header"),
            );
            Ok(response)
        },
    )
    .await
    .expect("accept websocket");
    let (device_id, worker_id) = connection_identity
        .lock()
        .expect("connection identity")
        .clone()
        .expect("captured connection identity");
    let expires_at_unix_ms = unix_ms().saturating_add(10_000);
    let server_key = test_server_signing_key();
    let challenge_signature = URL_SAFE_NO_PAD.encode(
        server_key
            .sign(&server_challenge_signing_payload(
                &nonce,
                expires_at_unix_ms,
                TEST_SERVER_ID,
                &device_id,
                "pc-a",
                TunnelService::Mcp,
                &worker_id,
            ))
            .to_bytes(),
    );
    socket
        .send(Message::Text(
            serde_json::to_string(&ControlMessage::Challenge {
                nonce: nonce.clone(),
                expires_at_unix_ms,
                server_id: TEST_SERVER_ID.into(),
                server_signature: challenge_signature,
            })
            .expect("challenge json")
            .into(),
        ))
        .await
        .expect("send challenge");
    let authenticate = socket
        .next()
        .await
        .expect("authenticate frame")
        .expect("authenticate frame");
    let Message::Text(authenticate) = authenticate else {
        panic!("expected authenticate text");
    };
    let ControlMessage::Authenticate(proof) =
        serde_json::from_str::<ControlMessage>(authenticate.as_ref()).expect("authenticate json")
    else {
        panic!("expected authenticate control");
    };
    let ack_signature = URL_SAFE_NO_PAD.encode(
        server_key
            .sign(&server_ack_signing_payload(
                &nonce,
                TEST_SERVER_ID,
                &proof,
                &policy,
            ))
            .to_bytes(),
    );
    socket
        .send(Message::Text(
            serde_json::to_string(&ControlMessage::HelloAck {
                protocol_version: PROTOCOL_VERSION,
                worker_policy: policy,
                server_id: TEST_SERVER_ID.into(),
                server_signature: ack_signature,
            })
            .expect("hello ack json")
            .into(),
        ))
        .await
        .expect("send hello ack");
    socket
}

#[test]
fn parses_namespaced_mcp_endpoint() {
    let endpoint = parse_builtin_endpoint(
        "https://tunnel.example.com/builtin/clients/pc-a/mcp",
        TunnelService::Mcp,
    )
    .unwrap();
    assert_eq!(endpoint.client_id, "pc-a");
    assert_eq!(endpoint.route_prefix, "/builtin/clients/pc-a");
    assert_eq!(
        endpoint.websocket_url,
        "wss://tunnel.example.com/_tunnel/v1"
    );
}

#[test]
fn upgrades_namespaced_mcp_base_url_to_endpoint() {
    let endpoint = parse_builtin_endpoint(
        "https://tunnel.example.com/builtin/clients/pc-a",
        TunnelService::Mcp,
    )
    .unwrap();
    assert_eq!(
        endpoint.public_url,
        "https://tunnel.example.com/builtin/clients/pc-a/mcp"
    );
    assert_eq!(endpoint.client_id, "pc-a");
}

#[test]
fn replaces_bootstrap_client_id_with_server_assigned_id() {
    let endpoint = builtin_endpoint_for_client(
        "https://tunnel.example.com/builtin/clients/workspace-placeholder/mcp",
        TunnelService::Mcp,
        "pc-a",
    )
    .unwrap();
    assert_eq!(
        endpoint.public_url,
        "https://tunnel.example.com/builtin/clients/pc-a/mcp"
    );
    assert_eq!(endpoint.client_id, "pc-a");
}

#[test]
fn actions_requests_strip_only_the_registered_prefix() {
    let config = BuiltinTunnelConfig {
        public_url: "https://example.com/builtin/actions/pc-a".into(),
        websocket_url: "wss://example.com/_tunnel/v1".into(),
        client_id: "pc-a".into(),
        service: TunnelService::Actions,
        route_prefix: "/builtin/actions/pc-a".into(),
        local_base_url: "http://127.0.0.1:7001".into(),
        device_id: "device-1".into(),
        signing_key: Arc::new(SigningKey::from_bytes(&[7_u8; 32])),
        server_id: TEST_SERVER_ID.into(),
        server_verifying_key: test_server_verifying_key(),
        log_path: PathBuf::new(),
    };
    assert_eq!(
        local_path_for_request(&config, "/builtin/actions/pc-a/openapi.json?x=1").unwrap(),
        "/openapi.json?x=1"
    );
    assert!(local_path_for_request(&config, "/builtin/actions/pc-ab").is_err());
}

#[test]
fn enrollment_link_must_match_the_public_origin_and_path() {
    let url = parse_enrollment_url(
        "https://tunnel.example.com/builtin/clients/pc-a/mcp",
        "https://tunnel.example.com/_tunnel/enroll/abc123",
    )
    .unwrap();
    assert_eq!(url.path(), "/_tunnel/enroll/abc123");
    assert!(parse_enrollment_url(
        "https://tunnel.example.com/builtin/clients/pc-a/mcp",
        "https://other.example/_tunnel/enroll/abc123",
    )
    .is_err());
    assert!(parse_enrollment_url(
        "https://tunnel.example.com/builtin/clients/pc-a/mcp",
        "https://tunnel.example.com/_tunnel/enroll/abc123?copy=1",
    )
    .is_err());
}

#[test]
fn rejects_non_namespaced_builtin_urls() {
    assert!(
        parse_builtin_endpoint("https://example.com/clients/pc-a/mcp", TunnelService::Mcp).is_err()
    );
}

#[test]
fn server_policy_updates_pool_metrics() {
    let metrics = BuiltinTunnelMetrics::new(1);
    let mut policy = WorkerPolicy::default_for(TunnelService::Mcp);
    policy.max_workers = 24;
    policy.revision = 7;
    metrics.set_policy(&policy);
    metrics.set_pool_counts(3, 2);

    let snapshot = metrics.snapshot();
    assert_eq!(snapshot.configured_workers, 24);
    assert_eq!(snapshot.idle_workers, 3);
    assert_eq!(snapshot.busy_workers, 2);
    assert_eq!(snapshot.policy_revision, 7);
}

#[test]
fn reconnect_backoff_is_jittered_bounded_and_resets_after_connect() {
    let base = Duration::from_secs(8);
    let worker_zero = reconnect_delay(base, 0, 3);
    let worker_one = reconnect_delay(base, 1, 3);

    assert!(worker_zero >= Duration::from_millis(6_400));
    assert!(worker_zero <= base);
    assert!(worker_one >= Duration::from_millis(6_400));
    assert!(worker_one <= base);
    assert_ne!(worker_zero, worker_one);
    assert_eq!(next_reconnect_base(base, true), ESTABLISHED_RECONNECT_DELAY);
    let established = reconnect_delay(ESTABLISHED_RECONNECT_DELAY, 0, 0);
    assert!(established >= Duration::from_millis(200));
    assert!(established <= ESTABLISHED_RECONNECT_DELAY);
    assert_eq!(
        next_reconnect_base(Duration::from_secs(10), false),
        MAX_RECONNECT_DELAY
    );
}

#[test]
fn connected_worker_guard_keeps_live_count_exact() {
    let metrics = Arc::new(BuiltinTunnelMetrics::new(8));
    assert_eq!(metrics.snapshot().connected_workers, 0);

    let first = ConnectedWorkerGuard::new(metrics.clone());
    let second = ConnectedWorkerGuard::new(metrics.clone());
    assert_eq!(metrics.snapshot().connected_workers, 2);

    drop(first);
    assert_eq!(metrics.snapshot().connected_workers, 1);
    drop(second);
    assert_eq!(metrics.snapshot().connected_workers, 0);
    assert_eq!(metrics.snapshot().configured_workers, 8);
}

#[test]
fn availability_state_distinguishes_running_from_reconnecting() {
    let reconnecting = BuiltinTunnelSnapshot {
        configured_workers: 8,
        connected_workers: 0,
        idle_workers: 0,
        busy_workers: 0,
        recycled_workers: 0,
        policy_revision: 1,
        last_error: Some("offline".into()),
    };
    let running = BuiltinTunnelSnapshot {
        configured_workers: 8,
        connected_workers: 2,
        idle_workers: 1,
        busy_workers: 1,
        recycled_workers: 3,
        policy_revision: 2,
        last_error: None,
    };

    assert_eq!(reconnecting.availability_state(true), "reconnecting");
    assert_eq!(running.availability_state(true), "running");
    assert_eq!(running.availability_state(false), "stopped");
}

#[test]
fn heartbeat_deadline_moves_forward_with_server_activity() {
    let started = Instant::now();
    let mut heartbeat = HeartbeatTracker::new_at(started);

    assert!(!heartbeat.expired_at(started + Duration::from_secs(44)));
    assert!(heartbeat.expired_at(started + Duration::from_secs(45)));

    heartbeat.record_activity_at(started + Duration::from_secs(30));
    assert!(!heartbeat.expired_at(started + Duration::from_secs(60)));
    assert!(heartbeat.expired_at(started + Duration::from_secs(75)));
}

#[test]
fn dynamic_pool_plan_uses_demand_connecting_limits_and_staged_shrink() {
    let policy = coding_tools_tunnel_protocol::WorkerPolicy::default_for(TunnelService::Mcp);
    let max_connecting = configured_max_connecting(&policy);
    assert_eq!(max_connecting, 4);
    assert_eq!(configured_burst_warm_floor(&policy), 8);

    assert_eq!(
        pool_adjustment(
            &policy,
            PoolCounts {
                total: 1,
                connecting: 1,
                idle: 0,
                busy: 0,
            },
            1,
            max_connecting,
            0,
            false,
            4,
        ),
        PoolAdjustment {
            spawn: 3,
            retire: 0,
        }
    );
    assert_eq!(
        pool_adjustment(
            &policy,
            PoolCounts {
                total: 4,
                connecting: 0,
                idle: 1,
                busy: 3,
            },
            0,
            max_connecting,
            16,
            false,
            8,
        ),
        PoolAdjustment {
            spawn: 4,
            retire: 0,
        }
    );
    let connecting_limited = PoolCounts {
        total: 8,
        connecting: 4,
        idle: 0,
        busy: 4,
    };
    let connecting_adjustment =
        pool_adjustment(&policy, connecting_limited, 4, max_connecting, 16, false, 8);
    assert_eq!(connecting_adjustment.spawn, 0);
    assert_eq!(
        scale_up_block(
            &policy,
            connecting_limited,
            4,
            max_connecting,
            16,
            connecting_adjustment,
        ),
        Some(ScaleUpBlock::ConnectingLimitReached)
    );

    for (total, floor, expected_retire) in [(16, 8, 4), (12, 8, 4), (8, 8, 0), (8, 4, 4)] {
        assert_eq!(
            pool_adjustment(
                &policy,
                PoolCounts {
                    total,
                    connecting: 0,
                    idle: total,
                    busy: 0,
                },
                0,
                max_connecting,
                0,
                true,
                floor,
            )
            .retire,
            expected_retire,
        );
    }

    let maximum = PoolCounts {
        total: usize::from(policy.max_workers),
        connecting: 0,
        idle: 0,
        busy: usize::from(policy.max_workers),
    };
    assert_eq!(
        scale_up_block(
            &policy,
            maximum,
            0,
            max_connecting,
            usize::from(policy.max_workers).saturating_add(1),
            PoolAdjustment {
                spawn: 0,
                retire: 0,
            },
        ),
        Some(ScaleUpBlock::MaxWorkersReached)
    );
}

#[test]
fn stale_connecting_workers_stop_counting_as_idle_reserve() {
    let (_retire_tx, retire_rx) = watch::channel(false);
    let (second_tx, _second_rx) = watch::channel(false);
    let now = Instant::now();
    let workers = HashMap::from([
        (
            1,
            ManagedWorker {
                state: PoolWorkerState::Connecting,
                connecting_since: now,
                retire: _retire_tx,
            },
        ),
        (
            2,
            ManagedWorker {
                state: PoolWorkerState::Connecting,
                connecting_since: now - Duration::from_secs(2),
                retire: second_tx,
            },
        ),
    ]);
    drop(retire_rx);
    assert_eq!(
        effective_connecting_workers(&workers, Duration::from_secs(1)),
        1
    );
    let policy = WorkerPolicy::default_for(TunnelService::Mcp);
    let counts = pool_counts(&workers);
    let adjustment = pool_adjustment(
        &policy,
        counts,
        1,
        configured_max_connecting(&policy),
        4,
        false,
        usize::from(policy.max_idle_workers),
    );
    assert_eq!(adjustment.spawn, 2);
}

#[test]
fn worker_recycle_limits_are_jittered_and_checked_only_at_idle_boundaries() {
    let low = jittered_limit(500, 7, 10);
    let high = jittered_limit(500, 8, 10);
    assert!((450..=550).contains(&low));
    assert!((450..=550).contains(&high));
    assert_ne!(low, high);

    let policy = coding_tools_tunnel_protocol::WorkerPolicy::default_for(TunnelService::Mcp);
    assert!(!worker_should_recycle(
        &policy,
        7,
        449,
        Duration::from_secs(10)
    ));
    assert!(worker_should_recycle(
        &policy,
        7,
        low,
        Duration::from_secs(10)
    ));
    assert!(worker_should_recycle(
        &policy,
        7,
        1,
        Duration::from_secs(jittered_limit(3_600, 7, 10))
    ));
}

#[tokio::test]
async fn worker_pool_bootstraps_grows_and_gracefully_shrinks_from_server_policy() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind websocket test server");
    let address = listener.local_addr().expect("test server address");
    let mut grow_policy = single_worker_policy();
    grow_policy.start_workers = 3;
    grow_policy.min_idle_workers = 2;
    grow_policy.max_idle_workers = 3;
    grow_policy.max_workers = 3;
    let (policy_tx, policy_rx) = watch::channel(grow_policy.clone());
    let (ready_tx, mut ready_rx) = mpsc::channel(3);
    let (closed_tx, mut closed_rx) = mpsc::channel(3);
    let server = tokio::spawn(async move {
        let mut handlers = JoinSet::new();
        for connection_index in 0..3 {
            let (stream, _) = listener.accept().await.expect("accept worker");
            let ready_tx = ready_tx.clone();
            let closed_tx = closed_tx.clone();
            let mut updates = policy_rx.clone();
            handlers.spawn(async move {
                let initial_policy = updates.borrow().clone();
                let mut socket = accept_authenticated_test_worker(
                    stream,
                    format!("grow-{connection_index}"),
                    initial_policy,
                )
                .await;
                let ready = socket.next().await.expect("ready frame").expect("ready");
                assert!(matches!(ready, Message::Text(_)));
                ready_tx.send(()).await.expect("report ready");

                updates.changed().await.expect("policy update");
                let updated_policy = updates.borrow().clone();
                socket
                    .send(Message::Text(
                        serde_json::to_string(&ControlMessage::PolicyUpdate {
                            worker_policy: updated_policy,
                        })
                        .expect("policy update json")
                        .into(),
                    ))
                    .await
                    .expect("send policy update");
                while let Some(message) = socket.next().await {
                    if matches!(message, Ok(Message::Close(_))) {
                        break;
                    }
                }
                let _ = closed_tx.send(()).await;
            });
        }
        drop(ready_tx);
        drop(closed_tx);
        while handlers.join_next().await.is_some() {}
    });

    let log_dir = tempfile::tempdir().expect("log tempdir");
    let config = BuiltinTunnelConfig {
        public_url: format!("http://{address}/builtin/clients/pc-a/mcp"),
        websocket_url: format!("ws://{address}{WS_PATH}"),
        client_id: "pc-a".into(),
        service: TunnelService::Mcp,
        route_prefix: "/builtin/clients/pc-a".into(),
        local_base_url: "http://127.0.0.1:1".into(),
        device_id: "device-1".into(),
        signing_key: Arc::new(SigningKey::from_bytes(&[29_u8; 32])),
        server_id: TEST_SERVER_ID.into(),
        server_verifying_key: test_server_verifying_key(),
        log_path: log_dir.path().join("builtin-grow-test.log"),
    };
    let metrics = Arc::new(BuiltinTunnelMetrics::new(1));
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (status_tx, _status_rx) = mpsc::channel(8);
    let pool_log_path = config.log_path.clone();
    let pool = tokio::spawn(run_worker_pool(
        config,
        shutdown_rx,
        status_tx,
        metrics.clone(),
    ));

    timeout(Duration::from_secs(4), async {
        for _ in 0..3 {
            ready_rx.recv().await.expect("worker ready");
        }
    })
    .await
    .expect("pool growth deadline");
    assert_eq!(metrics.snapshot().configured_workers, 3);
    assert_eq!(metrics.snapshot().connected_workers, 3);

    let mut shrink_policy = single_worker_policy();
    shrink_policy.revision = 2;
    policy_tx
        .send(shrink_policy)
        .expect("publish shrink policy");
    timeout(Duration::from_secs(4), async {
        while metrics.snapshot().connected_workers > 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        for _ in 0..2 {
            closed_rx.recv().await.expect("retired worker");
        }
    })
    .await
    .expect("pool shrink deadline");
    assert_eq!(metrics.snapshot().configured_workers, 1);
    assert_eq!(metrics.snapshot().connected_workers, 1);

    let _ = shutdown_tx.send(true);
    timeout(Duration::from_secs(2), pool)
        .await
        .expect("pool shutdown")
        .expect("pool task");
    timeout(Duration::from_secs(2), server)
        .await
        .expect("server shutdown")
        .expect("server task");
    let pool_log = std::fs::read_to_string(pool_log_path).expect("pool audit log");
    assert!(pool_log.contains("event=worker_policy_applied"));
    assert!(pool_log.contains("event=scale_up"));
    assert!(pool_log.contains("reason=startup"));
    assert!(pool_log.contains("event=scale_down"));
    assert!(pool_log.contains("reason=max_workers_reduced"));
}

#[tokio::test]
async fn worker_recycles_after_request_limit_and_pool_replaces_it() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let local_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local http server");
    let local_address = local_listener.local_addr().expect("local http address");
    let local_server = tokio::spawn(async move {
        let (mut stream, _) = local_listener.accept().await.expect("accept local request");
        let mut request = vec![0_u8; 4096];
        let read = stream.read(&mut request).await.expect("read local request");
        assert!(String::from_utf8_lossy(&request[..read]).starts_with("GET "));
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .await
            .expect("write local response");
    });

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind websocket test server");
    let address = listener.local_addr().expect("test server address");
    let mut recycle_policy = single_worker_policy();
    recycle_policy.max_requests_per_worker = 1;
    recycle_policy.max_lifetime_seconds = 0;
    recycle_policy.recycle_jitter_percent = 0;
    let (replacement_tx, mut replacement_rx) = mpsc::channel(1);
    let server = tokio::spawn(async move {
        for connection_index in 0..2 {
            let (stream, _) = listener.accept().await.expect("accept worker");
            let mut socket = accept_authenticated_test_worker(
                stream,
                format!("recycle-{connection_index}"),
                recycle_policy.clone(),
            )
            .await;
            let ready = socket.next().await.expect("ready frame").expect("ready");
            assert_eq!(
                serde_json::from_str::<ControlMessage>(ready.into_text().unwrap().as_ref())
                    .expect("ready json"),
                ControlMessage::Ready
            );

            if connection_index == 0 {
                socket
                    .send(Message::Text(
                        serde_json::to_string(&ControlMessage::RequestHead {
                            request_id: "request-1".into(),
                            method: "GET".into(),
                            path_and_query: "/builtin/clients/pc-a/mcp".into(),
                            headers: Vec::new(),
                            demand: None,
                        })
                        .expect("request head")
                        .into(),
                    ))
                    .await
                    .expect("send request head");
                socket
                    .send(Message::Text(
                        serde_json::to_string(&ControlMessage::RequestEnd {
                            request_id: "request-1".into(),
                        })
                        .expect("request end")
                        .into(),
                    ))
                    .await
                    .expect("send request end");
                let mut response_finished = false;
                while let Some(message) = socket.next().await {
                    match message.expect("response frame") {
                        Message::Text(text) => {
                            let control = serde_json::from_str::<ControlMessage>(&text)
                                .expect("response control");
                            if matches!(control, ControlMessage::ResponseEnd { .. }) {
                                response_finished = true;
                            }
                        }
                        Message::Close(_) => break,
                        _ => {}
                    }
                }
                assert!(response_finished);
            } else {
                replacement_tx.send(()).await.expect("replacement ready");
                while socket.next().await.is_some() {}
            }
        }
    });

    let log_dir = tempfile::tempdir().expect("log tempdir");
    let config = BuiltinTunnelConfig {
        public_url: format!("http://{address}/builtin/clients/pc-a/mcp"),
        websocket_url: format!("ws://{address}{WS_PATH}"),
        client_id: "pc-a".into(),
        service: TunnelService::Mcp,
        route_prefix: "/builtin/clients/pc-a".into(),
        local_base_url: format!("http://{local_address}"),
        device_id: "device-1".into(),
        signing_key: Arc::new(SigningKey::from_bytes(&[31_u8; 32])),
        server_id: TEST_SERVER_ID.into(),
        server_verifying_key: test_server_verifying_key(),
        log_path: log_dir.path().join("builtin-recycle-test.log"),
    };
    let metrics = Arc::new(BuiltinTunnelMetrics::new(1));
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (status_tx, _status_rx) = mpsc::channel(8);
    let pool = tokio::spawn(run_worker_pool(
        config,
        shutdown_rx,
        status_tx,
        metrics.clone(),
    ));

    timeout(Duration::from_secs(4), replacement_rx.recv())
        .await
        .expect("replacement deadline")
        .expect("replacement worker");
    assert_eq!(metrics.snapshot().recycled_workers, 1);
    assert_eq!(metrics.snapshot().connected_workers, 1);

    let _ = shutdown_tx.send(true);
    timeout(Duration::from_secs(2), pool)
        .await
        .expect("pool shutdown")
        .expect("pool task");
    timeout(Duration::from_secs(2), server)
        .await
        .expect("server shutdown")
        .expect("server task");
    timeout(Duration::from_secs(2), local_server)
        .await
        .expect("local server shutdown")
        .expect("local server task");
}

#[tokio::test]
async fn worker_pool_reconnects_after_authenticated_socket_closes() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind websocket test server");
    let address = listener.local_addr().expect("test server address");
    let server = tokio::spawn(async move {
        for connection_index in 0..2 {
            let (stream, _) = listener.accept().await.expect("accept worker");
            let mut socket = accept_authenticated_test_worker(
                stream,
                format!("nonce-{connection_index}"),
                single_worker_policy(),
            )
            .await;
            let ready = socket
                .next()
                .await
                .expect("ready frame")
                .expect("ready frame");
            let Message::Text(ready) = ready else {
                panic!("expected ready text");
            };
            assert_eq!(
                serde_json::from_str::<ControlMessage>(ready.as_ref()).expect("ready json"),
                ControlMessage::Ready
            );

            if connection_index == 0 {
                socket.close(None).await.expect("close first socket");
            } else {
                while socket.next().await.is_some() {}
            }
        }
    });

    let log_dir = tempfile::tempdir().expect("log tempdir");
    let config = BuiltinTunnelConfig {
        public_url: format!("http://{address}/builtin/clients/pc-a/mcp"),
        websocket_url: format!("ws://{address}{WS_PATH}"),
        client_id: "pc-a".into(),
        service: TunnelService::Mcp,
        route_prefix: "/builtin/clients/pc-a".into(),
        local_base_url: "http://127.0.0.1:1".into(),
        device_id: "device-1".into(),
        signing_key: Arc::new(SigningKey::from_bytes(&[23_u8; 32])),
        server_id: TEST_SERVER_ID.into(),
        server_verifying_key: test_server_verifying_key(),
        log_path: log_dir.path().join("builtin-reconnect-test.log"),
    };
    let metrics = Arc::new(BuiltinTunnelMetrics::new(1));
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (status_tx, mut status_rx) = mpsc::channel(8);
    let pool = tokio::spawn(run_worker_pool(
        config,
        shutdown_rx,
        status_tx,
        metrics.clone(),
    ));

    assert_eq!(status_rx.recv().await.expect("first connection"), Ok(()));
    let saw_reconnect = timeout(Duration::from_secs(3), async {
        let mut saw_disconnect = false;
        loop {
            match status_rx.recv().await.expect("worker status") {
                Ok(()) if saw_disconnect => return true,
                Ok(()) => {}
                Err(_) => saw_disconnect = true,
            }
        }
    })
    .await
    .expect("reconnect deadline");
    assert!(saw_reconnect);
    assert_eq!(metrics.snapshot().connected_workers, 1);

    let _ = shutdown_tx.send(true);
    timeout(Duration::from_secs(2), pool)
        .await
        .expect("pool shutdown")
        .expect("pool task");
    timeout(Duration::from_secs(2), server)
        .await
        .expect("server shutdown")
        .expect("server task");
}
