use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sha2::{Digest, Sha256};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::oneshot;
use tokio::time;

use crate::error::{AppError, AppResult};
use crate::platform::platform;
use crate::settings::ProxyConfig;

const READY_TIMEOUT: Duration = Duration::from_secs(45);
const EDGE_PROBE_HOST: &str = "region1.v2.argotunnel.com";

/// True for addresses handed out by TUN-mode proxy software in Fake-IP mode
/// (Clash / FlClash / Mihomo / sing-box): 198.18.0.0/15 (RFC 2544 benchmark
/// range, never a real public address) and Mihomo's default fdfe:dcba:9876::/48.
pub(crate) fn is_fake_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            octets[0] == 198 && (octets[1] == 18 || octets[1] == 19)
        }
        IpAddr::V6(v6) => {
            let segments = v6.segments();
            segments[0] == 0xfdfe && segments[1] == 0xdcba && segments[2] == 0x9876
        }
    }
}

/// Resolve the Cloudflare edge host and report whether DNS is being answered
/// by a TUN/Fake-IP proxy. Failures are treated as "not detected".
async fn fake_ip_dns_detected() -> bool {
    match time::timeout(
        Duration::from_secs(3),
        tokio::net::lookup_host((EDGE_PROBE_HOST, 7844u16)),
    )
    .await
    {
        Ok(Ok(addrs)) => addrs.into_iter().any(|addr| is_fake_ip(addr.ip())),
        _ => false,
    }
}

/// Map the configured transport to a cloudflared `--protocol` value.
/// HTTP/2 over TCP is the default: QUIC (UDP 7844) is frequently dropped or
/// mangled by TUN-mode proxies and restrictive networks, which makes
/// cloudflared spend ~100 s timing out before it falls back.
pub(crate) fn resolve_tunnel_protocol(configured: &str, fake_ip: bool) -> Option<&'static str> {
    match configured.trim().to_ascii_lowercase().as_str() {
        "quic" => Some("quic"),
        "auto" if !fake_ip => None,
        _ => Some("http2"),
    }
}

const TUN_HINT: &str = "检测到 Clash / FlClash / Mihomo 等代理软件的 TUN（虚拟网卡）+ Fake-IP 模式，已自动改用 HTTP/2 连接。\n\
若仍无法连接：在代理软件中将 argotunnel.com、trycloudflare.com、cloudflare.com 设为直连（DIRECT），\n\
并把 +.argotunnel.com、+.trycloudflare.com 加入 fake-ip-filter（或 DNS 改用 redir-host），然后重新开始。";

/// Handle to a supervised `cloudflared` child process.
pub struct CloudflareTunnelHandle {
    pub child: Child,
    pub public_url: String,
    pub pid: Option<u32>,
}

pub fn resolve_cloudflared() -> AppResult<PathBuf> {
    platform()
        .cloudflared_candidates()
        .into_iter()
        .find(|path| path.is_file())
        .or_else(|| cached_cloudflared_path().filter(|path| path.is_file()))
        .ok_or_else(|| {
            AppError::Message(
                "未找到 cloudflared。请到「软件管理」安装，或自行安装 Cloudflare Tunnel CLI。\n\
                 Windows 可执行：winget install Cloudflare.cloudflared"
                    .into(),
            )
        })
}

/// Path where the app caches a self-managed cloudflared binary.
pub(crate) fn cached_cloudflared_path() -> Option<PathBuf> {
    platform()
        .app_config_dir()
        .ok()
        .map(|dir| dir.join("bin").join(cloudflared_binary_name()))
}

pub(crate) fn cloudflared_binary_name() -> &'static str {
    #[cfg(windows)]
    {
        "cloudflared.exe"
    }
    #[cfg(not(windows))]
    {
        "cloudflared"
    }
}

/// GitHub release asset name for the current platform.
fn cloudflared_release_asset() -> AppResult<&'static str> {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        Ok("cloudflared-windows-amd64.exe")
    }
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    {
        Err(AppError::Message(
            "cloudflared 2025.6.1 未发布可校验的 Windows ARM64 自动下载资产，请使用系统安装的 cloudflared。".into(),
        ))
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        Ok("cloudflared-linux-amd64")
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        Ok("cloudflared-linux-arm64")
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        Ok("cloudflared-darwin-amd64.tgz")
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        Ok("cloudflared-darwin-arm64.tgz")
    }
    #[cfg(not(any(
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "aarch64"),
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64"),
    )))]
    {
        Err(AppError::Message(
            "当前平台暂不支持自动下载 cloudflared。".into(),
        ))
    }
}

/// Latest cloudflared release. Pinned for reproducibility; bump as needed.
const CLOUDFLARED_VERSION: &str = "2025.6.1";

fn expected_cloudflared_sha256(asset: &str) -> AppResult<&'static str> {
    match asset {
        "cloudflared-windows-amd64.exe" => {
            Ok("a4af4d26a86ed48f43647d151be37b0907f15c3ac230f0ab95aa226b3e0b8803")
        }
        "cloudflared-linux-amd64" => {
            Ok("103ff020ffcc4ad6b542948b95ecff417150c70a17bff3a39ac2670b4159c9bb")
        }
        "cloudflared-linux-arm64" => {
            Ok("87a38f8b0c371b926224a1346443096a8b9f38138561e0b314efa4c9fc1f51f7")
        }
        "cloudflared-darwin-amd64.tgz" => {
            Ok("b81b684ff28bd614d048559ba5e45892fb9cdb69347ca83c418dd4386b6e4735")
        }
        "cloudflared-darwin-arm64.tgz" => {
            Ok("9cc4c04b3cec473c3bf4342a7b5b6628358953e568fd4682aadb390bac85a23a")
        }
        _ => Err(AppError::Message(format!(
            "cloudflared {CLOUDFLARED_VERSION} 资产缺少固定 SHA-256：{asset}"
        ))),
    }
}

fn verify_cloudflared_download(asset: &str, bytes: &[u8]) -> AppResult<()> {
    let expected = expected_cloudflared_sha256(asset)?;
    let actual = format!("{:x}", Sha256::digest(bytes));
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(AppError::Message(format!(
            "cloudflared 下载完整性校验失败：{asset} SHA-256 不匹配"
        )));
    }
    Ok(())
}

/// Download cloudflared into the app cache `bin/` directory, honoring the
/// configured mirror + proxy. Windows/Linux assets are raw binaries; macOS
/// assets are `.tgz` archives that need extraction.
pub(crate) async fn download_cloudflared_to_cache() -> AppResult<PathBuf> {
    let settings = crate::settings::AppSettings::load_or_default();
    let asset = cloudflared_release_asset()?;
    let url = format!(
        "https://github.com/cloudflare/cloudflared/releases/download/{CLOUDFLARED_VERSION}/{asset}"
    );
    let dest =
        cached_cloudflared_path().ok_or_else(|| AppError::Message("无法解析缓存目录。".into()))?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let bytes =
        crate::tunnel::download::download_release_asset(&settings, &url, "cloudflared").await?;
    verify_cloudflared_download(asset, &bytes)?;

    if asset.ends_with(".tgz") {
        extract_cloudflared_from_tar_gz(&bytes, &dest)?;
    } else {
        std::fs::write(&dest, &bytes)?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&dest) {
            let mut perms = meta.permissions();
            perms.set_mode(0o755);
            let _ = std::fs::set_permissions(&dest, perms);
        }
    }

    if dest.is_file() {
        Ok(dest)
    } else {
        Err(AppError::Message("cloudflared 自动安装失败。".into()))
    }
}

#[cfg(target_os = "macos")]
fn extract_cloudflared_from_tar_gz(bytes: &[u8], dest: &Path) -> AppResult<()> {
    let decoder = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(decoder);
    for entry in archive
        .entries()
        .map_err(|err| AppError::Message(format!("解压 cloudflared 安装包失败: {err}")))?
    {
        let mut entry = entry
            .map_err(|err| AppError::Message(format!("读取 cloudflared 安装包失败: {err}")))?;
        let path = entry
            .path()
            .map_err(|err| AppError::Message(err.to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        if path.ends_with("cloudflared") {
            let mut out = std::fs::File::create(dest)?;
            std::io::copy(&mut entry, &mut out)?;
            return Ok(());
        }
    }
    Err(AppError::Message(
        "cloudflared 安装包中未找到可执行文件。".into(),
    ))
}

#[cfg(not(target_os = "macos"))]
#[allow(dead_code)]
fn extract_cloudflared_from_tar_gz(_bytes: &[u8], _dest: &Path) -> AppResult<()> {
    Err(AppError::Message(
        "当前平台的 cloudflared 无需解压。".into(),
    ))
}

pub fn extract_trycloudflare_url(line: &str) -> Option<String> {
    const PREFIX: &str = "https://";
    const SUFFIX: &str = ".trycloudflare.com";
    let lower = line.to_ascii_lowercase();
    let mut search_from = 0;

    while let Some(rel) = lower[search_from..].find(PREFIX) {
        let start = search_from + rel;
        let Some(suffix_rel) = lower[start..].find(SUFFIX) else {
            break;
        };
        let end = start + suffix_rel + SUFFIX.len();
        let host = &line[start + PREFIX.len()..end - SUFFIX.len()];
        if host.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') && !host.is_empty() {
            return Some(line[start..end].trim_end_matches('/').to_string());
        }
        search_from = start + PREFIX.len();
    }
    None
}

/// Apply the global proxy to a tunnel child process environment.
pub(crate) fn apply_proxy_env(cmd: &mut Command, proxy: &ProxyConfig) {
    let url = match proxy.mode.as_str() {
        "manual" if !proxy.url.trim().is_empty() => Some(proxy.url.trim().to_string()),
        "system" => std::env::var("HTTPS_PROXY")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| std::env::var("HTTP_PROXY").ok().filter(|s| !s.is_empty()))
            .or_else(|| std::env::var("ALL_PROXY").ok().filter(|s| !s.is_empty())),
        _ => None,
    };
    if let Some(url) = url {
        for key in [
            "HTTPS_PROXY",
            "HTTP_PROXY",
            "https_proxy",
            "http_proxy",
            "ALL_PROXY",
            "all_proxy",
        ] {
            cmd.env(key, &url);
        }
        // Some cloudflared builds consult this dedicated variable.
        cmd.env("TUNNEL_HTTP_PROXY", &url);
    }
}

/// Spawn a quick tunnel to the configured local listener or run a named tunnel.
pub async fn spawn_cloudflare_tunnel(
    port: u16,
    bind_address: &str,
    cwd: &Path,
    log_path: &Path,
    cloudflare_mode: &str,
    cloudflare_token: &str,
    named_public_url: &str,
    use_proxy: bool,
) -> AppResult<CloudflareTunnelHandle> {
    let cloudflared = resolve_cloudflared()?;
    let quick = cloudflare_mode != "named";

    if !quick {
        if cloudflare_token.trim().is_empty() {
            return Err(AppError::Message(
                "Cloudflare 命名隧道模式需要填写 Tunnel Token。".into(),
            ));
        }
        if named_public_url.trim().is_empty() {
            return Err(AppError::Message(
                "Cloudflare 命名隧道模式需要填写固定公网地址。".into(),
            ));
        }
    }

    let mut cmd = Command::new(&cloudflared);
    cmd.current_dir(cwd);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    #[cfg(windows)]
    {
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }

    #[cfg(unix)]
    {
        cmd.process_group(0);
    }

    let settings = crate::settings::AppSettings::load_or_default();
    if use_proxy {
        apply_proxy_env(&mut cmd, &settings.proxy);
    }

    let fake_ip = fake_ip_dns_detected().await;
    let protocol = resolve_tunnel_protocol(&settings.proxy.tunnel_protocol, fake_ip);
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    {
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
        {
            let _ = writeln!(
                file,
                "[coding-tools] cloudflared protocol={} fake_ip_dns={fake_ip}",
                protocol.unwrap_or("auto")
            );
        }
    }

    cmd.args(["tunnel", "--no-autoupdate"]);
    if let Some(protocol) = protocol {
        cmd.args(["--protocol", protocol]);
    }
    if quick {
        let local_url = format!(
            "http://{}:{port}",
            crate::workspace::url_host_for_bind(bind_address)
        );
        cmd.args(["--url", &local_url]);
    } else {
        cmd.args(["run", "--token", cloudflare_token.trim()]);
    }

    let mut child = cmd
        .spawn()
        .map_err(|err| AppError::Message(format!("启动 cloudflared 失败: {err}")))?;
    let pid = child.id();

    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let (ready_tx, ready_rx) = oneshot::channel();
    let log_path = log_path.to_path_buf();
    let named_url = named_public_url.trim_end_matches('/').to_string();
    let log_path_for_error = log_path.clone();
    let seen_url: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let seen_url_for_stream = seen_url.clone();

    if let Some(stdout) = child.stdout.take() {
        let stderr = child.stderr.take();
        tokio::spawn(async move {
            stream_cloudflare_output(
                stdout,
                stderr,
                &log_path,
                quick,
                named_url,
                seen_url_for_stream,
                ready_tx,
            )
            .await;
        });
    } else {
        let _ = ready_tx.send(QuickTunnelReady {
            public_url: if quick {
                None
            } else {
                Some(named_public_url.trim_end_matches('/').to_string())
            },
            named_ready: !quick,
        });
    }

    let tun_hint = if fake_ip {
        format!("\n{TUN_HINT}")
    } else {
        String::new()
    };
    let ready = match time::timeout(READY_TIMEOUT, ready_rx).await {
        Ok(Ok(ready)) => ready,
        Ok(Err(_)) => {
            return Err(AppError::Message(format!(
                "cloudflared 输出流意外结束。请查看日志：{}{tun_hint}",
                log_path_for_error.display()
            )))
        }
        Err(_) => {
            // The quick-tunnel URL is allocated before the edge connection is
            // registered. If we have a URL, hand it out: cloudflared keeps
            // retrying in the background and the supervisor reports health.
            let fallback = if quick {
                seen_url.lock().ok().and_then(|guard| guard.clone())
            } else {
                None
            };
            match fallback {
                Some(url) => QuickTunnelReady {
                    public_url: Some(url),
                    named_ready: false,
                },
                None => {
                    return Err(AppError::Message(format!(
                        "cloudflared 已启动，但在 {} 秒内没有连上 Cloudflare。\n\
                         请检查：1) MCP 服务是否已在本机端口 {port} 运行；2) 设置 → 通用 → 网络代理 是否配置正确（如 http://127.0.0.1:7890）；\
                         3) 查看日志 {log_hint}{tun_hint}",
                        READY_TIMEOUT.as_secs(),
                        log_hint = log_path_for_error.display()
                    )))
                }
            }
        }
    };

    let public_url = if quick {
        ready.public_url.ok_or_else(|| {
            AppError::Message(format!(
                "cloudflared 已启动，但没有解析到 trycloudflare.com 地址。请查看日志：{}",
                log_path_for_error.display()
            ))
        })?
    } else {
        named_public_url.trim_end_matches('/').to_string()
    };

    Ok(CloudflareTunnelHandle {
        child,
        public_url,
        pid,
    })
}

struct QuickTunnelReady {
    public_url: Option<String>,
    #[allow(dead_code)]
    named_ready: bool,
}

async fn stream_cloudflare_output<R, E>(
    stdout: R,
    stderr: Option<E>,
    log_path: &Path,
    quick: bool,
    named_url: String,
    seen_url: Arc<Mutex<Option<String>>>,
    ready_tx: oneshot::Sender<QuickTunnelReady>,
) where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    E: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let mut ready_tx = Some(ready_tx);
    let mut public_url: Option<String> = None;

    let mut log = match tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .await
    {
        Ok(file) => file,
        Err(_) => {
            if let Some(tx) = ready_tx.take() {
                let _ = tx.send(QuickTunnelReady {
                    public_url: if quick { None } else { Some(named_url) },
                    named_ready: !quick,
                });
            }
            return;
        }
    };

    let send_ready = |tx: &mut Option<oneshot::Sender<QuickTunnelReady>>,
                      url: Option<String>,
                      named_ready: bool| {
        if let Some(sender) = tx.take() {
            let _ = sender.send(QuickTunnelReady {
                public_url: url,
                named_ready,
            });
        }
    };

    let handle_line =
        |line: &str,
         public_url: &mut Option<String>,
         ready_tx: &mut Option<oneshot::Sender<QuickTunnelReady>>| {
            if quick {
                if public_url.is_none() {
                    if let Some(url) = extract_trycloudflare_url(line) {
                        *public_url = Some(url.clone());
                        if let Ok(mut guard) = seen_url.lock() {
                            *guard = Some(url);
                        }
                    }
                }
                // Only report ready once the edge connection is registered;
                // otherwise the URL answers with Cloudflare error 1033.
                if public_url.is_some()
                    && line
                        .to_ascii_lowercase()
                        .contains("registered tunnel connection")
                {
                    send_ready(ready_tx, public_url.clone(), true);
                }
            } else {
                let lowered = line.to_ascii_lowercase();
                if lowered.contains("registered tunnel connection")
                    || lowered.contains("starting metrics server")
                {
                    send_ready(ready_tx, Some(named_url.clone()), true);
                }
            }
        };

    // cloudflared logs primarily to stderr; read stdout and stderr concurrently.
    let (line_tx, mut line_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let stderr_line_tx = line_tx.clone();

    tokio::spawn(async move {
        let mut stdout = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = stdout.next_line().await {
            if line_tx.send(line).is_err() {
                break;
            }
        }
    });

    if let Some(stderr) = stderr {
        tokio::spawn(async move {
            let mut stderr = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = stderr.next_line().await {
                if stderr_line_tx.send(line).is_err() {
                    break;
                }
            }
        });
    }

    while let Some(line) = line_rx.recv().await {
        let _ = log.write_all(line.as_bytes()).await;
        let _ = log.write_all(b"\n").await;
        let _ = log.flush().await;
        handle_line(&line, &mut public_url, &mut ready_tx);
    }

    send_ready(&mut ready_tx, public_url, !quick);
}

pub async fn stop_child(mut child: Child, pid: Option<u32>) -> AppResult<()> {
    if let Some(pid) = pid {
        let _ = platform().terminate_process_tree(pid);
    }

    let _ = child.kill().await;
    let _ = time::timeout(Duration::from_secs(3), child.wait()).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{extract_trycloudflare_url, is_fake_ip, resolve_tunnel_protocol};

    #[test]
    fn detects_tun_fake_ip_ranges() {
        assert!(is_fake_ip("198.18.0.12".parse().unwrap()));
        assert!(is_fake_ip("198.19.255.1".parse().unwrap()));
        assert!(is_fake_ip("fdfe:dcba:9876::7e".parse().unwrap()));
        assert!(!is_fake_ip("198.41.192.7".parse().unwrap()));
        assert!(!is_fake_ip("2606:4700::1".parse().unwrap()));
    }

    #[test]
    fn defaults_to_http2_and_overrides_auto_under_fake_ip() {
        assert_eq!(resolve_tunnel_protocol("", false), Some("http2"));
        assert_eq!(resolve_tunnel_protocol("http2", false), Some("http2"));
        assert_eq!(resolve_tunnel_protocol("auto", false), None);
        assert_eq!(resolve_tunnel_protocol("auto", true), Some("http2"));
        assert_eq!(resolve_tunnel_protocol("QUIC", true), Some("quic"));
    }

    #[test]
    fn extracts_trycloudflare_url_from_log_line() {
        let line = "INF | https://abc-def.trycloudflare.com is your tunnel URL";
        assert_eq!(
            extract_trycloudflare_url(line).as_deref(),
            Some("https://abc-def.trycloudflare.com")
        );
    }

    #[test]
    fn ignores_invalid_hosts() {
        let line = "https://bad_host.trycloudflare.com";
        assert!(extract_trycloudflare_url(line).is_none());
    }
}
