use std::net::{IpAddr, Ipv4Addr};
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

const TUN_HINT: &str = "检测到 Clash / FlClash / Mihomo 等代理软件的 TUN（虚拟网卡）+ Fake-IP 模式，已自动改用 HTTP/2，并尝试绕过虚拟网卡直连 Cloudflare 边缘节点（真实 IP + 物理网卡）。\n\
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
            "cloudflared 未发布可校验的 Windows ARM64 自动下载资产，请使用系统安装的 cloudflared。"
                .into(),
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

/// Pinned cloudflared release (with fixed SHA-256 below). Cloudflare stops
/// supporting connectors more than a year behind the latest release, so keep
/// this current; cached copies older than this are replaced automatically.
pub(crate) const CLOUDFLARED_VERSION: &str = "2026.9.3";

fn expected_cloudflared_sha256(asset: &str) -> AppResult<&'static str> {
    match asset {
        "cloudflared-windows-amd64.exe" => {
            Ok("f096265ec2fcbe9bb6e2d64268db167ced3fcbb83d894bdb9e2fcdb26f2ea7e2")
        }
        "cloudflared-linux-amd64" => {
            Ok("77e26d8d900e0b8469f416239d14b5f296525fdf79fee6f511ef55609e3fbac2")
        }
        "cloudflared-linux-arm64" => {
            Ok("aaeb2d7d0da3614634c7e03ab13487a1522c2e79165ed2929cfe23d5e95b326d")
        }
        "cloudflared-darwin-amd64.tgz" => {
            Ok("d1155d0837487f261183b15c1eab6c4ebcad9dc49b94675f1524c3564cea3977")
        }
        "cloudflared-darwin-arm64.tgz" => {
            Ok("587c2cfb1c230fe36c7fa7727da78be459dae028cabe8c001291999350f07095")
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
    let result = download_cloudflared_inner().await;
    if let Ok(mut last) = LAST_DOWNLOAD_FAILURE.lock() {
        *last = result.is_err().then(std::time::Instant::now);
    }
    result
}

/// When the last download failed; automatic updates back off for a while so a
/// blocked network does not delay every tunnel start by the download timeout.
static LAST_DOWNLOAD_FAILURE: Mutex<Option<std::time::Instant>> = Mutex::new(None);
const DOWNLOAD_RETRY_BACKOFF: Duration = Duration::from_secs(600);

fn download_recently_failed() -> bool {
    LAST_DOWNLOAD_FAILURE
        .lock()
        .ok()
        .and_then(|last| *last)
        .is_some_and(|at| at.elapsed() < DOWNLOAD_RETRY_BACKOFF)
}

async fn download_cloudflared_inner() -> AppResult<PathBuf> {
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

    // Write next to the target first, then swap it in. On Windows a running
    // cloudflared.exe (e.g. another workspace's tunnel) cannot be overwritten
    // but can be renamed, so the old binary is moved aside instead.
    let staging = dest.with_file_name(format!("{}.download", cloudflared_binary_name()));
    let _ = std::fs::remove_file(&staging);
    if asset.ends_with(".tgz") {
        extract_cloudflared_from_tar_gz(&bytes, &staging)?;
    } else {
        std::fs::write(&staging, &bytes)?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&staging) {
            let mut perms = meta.permissions();
            perms.set_mode(0o755);
            let _ = std::fs::set_permissions(&staging, perms);
        }
    }

    swap_in_cloudflared(&staging, &dest)?;
    let _ = std::fs::write(cloudflared_version_marker(&dest), CLOUDFLARED_VERSION);

    if dest.is_file() {
        Ok(dest)
    } else {
        Err(AppError::Message("cloudflared 自动安装失败。".into()))
    }
}

fn cloudflared_version_marker(binary: &Path) -> PathBuf {
    binary.with_file_name("cloudflared.version")
}

/// Replace `dest` with `staging`, moving a busy old binary out of the way.
fn swap_in_cloudflared(staging: &Path, dest: &Path) -> AppResult<()> {
    let dir = dest.parent().map(Path::to_path_buf).unwrap_or_default();
    // Best-effort cleanup of binaries retired by earlier updates.
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("cloudflared") && name.contains(".old") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    if dest.exists() && std::fs::remove_file(dest).is_err() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let retired = dir.join(format!("{}.old-{stamp}", cloudflared_binary_name()));
        std::fs::rename(dest, &retired).map_err(|err| {
            AppError::Message(format!(
                "无法替换旧版 cloudflared（{}）：{err}",
                dest.display()
            ))
        })?;
    }
    std::fs::rename(staging, dest).map_err(|err| {
        AppError::Message(format!(
            "无法安装新版 cloudflared（{}）：{err}",
            dest.display()
        ))
    })
}

/// Parses `cloudflared version 2026.9.3 (built ...)` into (2026, 9, 3).
pub(crate) fn parse_cloudflared_version(text: &str) -> Option<(u32, u32, u32)> {
    let start = text
        .find("version ")
        .map(|i| i + "version ".len())
        .unwrap_or(0);
    let token = text[start..]
        .split_whitespace()
        .next()?
        .trim_start_matches('v');
    let mut parts = token.split('.').map(|part| {
        part.chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse::<u32>()
            .ok()
    });
    let year = parts.next()??;
    let month = parts.next()??;
    let patch = parts.next().flatten().unwrap_or(0);
    (year >= 2017).then_some((year, month, patch))
}

fn pinned_cloudflared_version() -> (u32, u32, u32) {
    parse_cloudflared_version(CLOUDFLARED_VERSION).unwrap_or((0, 0, 0))
}

/// Runs `<binary> --version` (bounded, no console window).
async fn probe_cloudflared_version(binary: &Path) -> Option<(u32, u32, u32)> {
    let mut cmd = Command::new(binary);
    cmd.arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let output = time::timeout(Duration::from_secs(8), cmd.output())
        .await
        .ok()?
        .ok()?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    parse_cloudflared_version(&text)
}

fn cached_cloudflared_version(binary: &Path) -> Option<(u32, u32, u32)> {
    std::fs::read_to_string(cloudflared_version_marker(binary))
        .ok()
        .and_then(|text| parse_cloudflared_version(text.trim()))
}

/// Version recorded for the app-managed copy and whether it predates the
/// pinned release. A copy without a marker was installed by an older app
/// version (<= 2025.6.1) and counts as outdated.
pub(crate) fn cached_cloudflared_state(binary: &Path) -> (String, bool) {
    match cached_cloudflared_version(binary) {
        Some(version) => (
            format_version(version),
            version < pinned_cloudflared_version(),
        ),
        None => (String::new(), true),
    }
}

fn format_version(version: (u32, u32, u32)) -> String {
    format!("{}.{}.{}", version.0, version.1, version.2)
}

/// Picks a cloudflared that Cloudflare still supports, updating the app-managed
/// copy when it (or the system copy) is older than the pinned release.
/// Never blocks on a failed update: the previous binary is used instead and a
/// note is returned for the tunnel log.
pub(crate) async fn ensure_cloudflared_current() -> AppResult<(PathBuf, Option<String>)> {
    static UPDATE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _guard = UPDATE_LOCK.lock().await;
    let pinned = pinned_cloudflared_version();

    let system = platform()
        .cloudflared_candidates()
        .into_iter()
        .find(|path| path.is_file());
    let mut system_version = None;
    if let Some(path) = &system {
        system_version = probe_cloudflared_version(path).await;
        if system_version.is_some_and(|version| version >= pinned) {
            return Ok((path.clone(), None));
        }
    }

    let cache = cached_cloudflared_path();
    if let Some(cache_path) = cache.as_ref().filter(|path| path.is_file()) {
        let version = match cached_cloudflared_version(cache_path) {
            Some(version) => Some(version),
            None => probe_cloudflared_version(cache_path).await,
        };
        if version.is_some_and(|version| version >= pinned) {
            return Ok((cache_path.clone(), None));
        }
    }

    let old = system
        .as_ref()
        .map(|path| (path.clone(), system_version))
        .or_else(|| {
            cache
                .as_ref()
                .filter(|path| path.is_file())
                .map(|path| (path.clone(), cached_cloudflared_version(path)))
        });
    let old_label = old
        .as_ref()
        .and_then(|(_, version)| *version)
        .map(format_version)
        .unwrap_or_else(|| "未知".into());

    if download_recently_failed() {
        if let Some((path, _)) = old.as_ref() {
            return Ok((
                path.clone(),
                Some(format!(
                    "cloudflared {old_label} 已过旧（最新 {CLOUDFLARED_VERSION}），最近一次自动更新失败，稍后会重试；暂时继续使用旧版本"
                )),
            ));
        }
    }

    match download_cloudflared_to_cache().await {
        Ok(path) => Ok((
            path,
            old.map(|_| {
                format!("cloudflared 已从 {old_label} 自动更新到 {CLOUDFLARED_VERSION}")
            }),
        )),
        Err(error) => match old {
            Some((path, _)) => Ok((
                path,
                Some(format!(
                    "cloudflared {old_label} 已过旧，自动更新到 {CLOUDFLARED_VERSION} 失败：{error}；暂时继续使用旧版本"
                )),
            )),
            None => Err(error),
        },
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
    let (cloudflared, update_note) = ensure_cloudflared_current().await?;
    let quick = cloudflare_mode != "named";
    let normalized_token = normalize_cloudflare_token(cloudflare_token);
    let cloudflare_token = normalized_token.as_str();

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

    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    if let Some(note) = &update_note {
        append_log_line(log_path, &format!("[coding-tools] {note}"));
    }

    let settings = crate::settings::AppSettings::load_or_default();
    let fake_ip = fake_ip_dns_detected().await;
    let routes = inspect_default_routes().await;
    let tun_detected = fake_ip || routes.tun_active;
    let protocol = resolve_tunnel_protocol(&settings.proxy.tunnel_protocol, tun_detected);
    append_log_line(
        log_path,
        &format!(
            "[coding-tools] cloudflared protocol={} fake_ip_dns={fake_ip} tun_route={} physical_ipv4={}",
            protocol.unwrap_or("auto"),
            routes.tun_active,
            routes
                .physical_ipv4
                .map(|ip| ip.to_string())
                .unwrap_or_else(|| "-".into())
        ),
    );

    let base = AttemptConfig {
        cloudflared: &cloudflared,
        cwd,
        log_path,
        quick,
        port,
        bind_address,
        cloudflare_token,
        named_public_url,
        proxy: if use_proxy {
            Some(&settings.proxy)
        } else {
            None
        },
        protocol,
    };

    let tun_hint = if tun_detected {
        format!("\n{TUN_HINT}")
    } else {
        String::new()
    };

    if tun_detected {
        // TUN mode: the proxy captures cloudflared's edge traffic (TCP/UDP 7844)
        // and most proxy nodes refuse that port, which surfaces as
        // "TLS handshake with edge error: EOF" and Cloudflare error 1033.
        // Dial the real edge IPs directly and, on Windows, bind the physical
        // adapter so the strong-host send model routes around the TUN adapter.
        let bypass = plan_tun_bypass(routes.physical_ipv4).await;
        append_log_line(
            log_path,
            &format!(
                "[coding-tools] TUN bypass: edges={} bind={}",
                bypass.edges.join(","),
                bypass
                    .bind
                    .map(|ip| ip.to_string())
                    .unwrap_or_else(|| "-".into())
            ),
        );
        match spawn_attempt(&base, Some(&bypass), BYPASS_READY_TIMEOUT, true).await {
            Ok(handle) => return Ok(handle),
            Err(err) => append_log_line(
                log_path,
                &format!("[coding-tools] TUN bypass failed, retrying normally: {err}"),
            ),
        }
    }

    // Under TUN, never hand out an unregistered URL: it would only answer
    // with Cloudflare error 1033.
    spawn_attempt(&base, None, READY_TIMEOUT, tun_detected)
        .await
        .map_err(|err| match err {
            AppError::Message(message) => AppError::Message(format!("{message}{tun_hint}")),
            other => other,
        })
}

const BYPASS_READY_TIMEOUT: Duration = Duration::from_secs(30);

/// Published Cloudflare Tunnel edge addresses (region1 / region2), used when
/// DNS-over-HTTPS cannot be reached.
const FALLBACK_EDGE_IPS: &[&str] = &[
    "198.41.192.7",
    "198.41.192.27",
    "198.41.192.47",
    "198.41.192.57",
    "198.41.192.67",
    "198.41.192.77",
    "198.41.200.13",
    "198.41.200.23",
    "198.41.200.33",
    "198.41.200.43",
    "198.41.200.53",
    "198.41.200.63",
];

struct TunBypass {
    edges: Vec<String>,
    bind: Option<Ipv4Addr>,
}

#[derive(Default)]
struct RouteInfo {
    physical_ipv4: Option<Ipv4Addr>,
    tun_active: bool,
}

/// Addresses used by TUN adapters of Clash-family proxies (FlClash defaults
/// to 172.19.0.1/30, Mihomo to 198.18.0.1/16) plus never-routable ranges.
pub(crate) fn is_tun_like_ipv4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    is_fake_ip(IpAddr::V4(ip))
        || (o[0] == 172 && o[1] == 19 && o[2] == 0)
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
}

/// Parse `route print -4` (Windows). The IPv4 route table columns are
/// "Network Destination, Netmask, Gateway, Interface, Metric" regardless of
/// the display language; the Interface column is the adapter's local IP.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn parse_route_print(text: &str) -> (Option<Ipv4Addr>, bool) {
    let mut best: Option<(u32, Ipv4Addr)> = None;
    let mut tun_active = false;
    for line in text.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 5 {
            continue;
        }
        let (Ok(dest), Ok(mask)) = (cols[0].parse::<Ipv4Addr>(), cols[1].parse::<Ipv4Addr>())
        else {
            continue;
        };
        let Ok(iface) = cols[3].parse::<Ipv4Addr>() else {
            continue;
        };
        let Ok(metric) = cols[4].parse::<u32>() else {
            continue;
        };
        let default_like = (dest.is_unspecified() && mask.is_unspecified())
            || ((dest == Ipv4Addr::new(0, 0, 0, 0) || dest == Ipv4Addr::new(128, 0, 0, 0))
                && mask == Ipv4Addr::new(128, 0, 0, 0));
        if !default_like {
            continue;
        }
        if is_tun_like_ipv4(iface) {
            tun_active = true;
            continue;
        }
        if !(dest.is_unspecified() && mask.is_unspecified()) {
            continue;
        }
        // "On-link" (localized) gateways are not physical uplinks.
        let Ok(gateway) = cols[2].parse::<Ipv4Addr>() else {
            continue;
        };
        if is_tun_like_ipv4(gateway) {
            tun_active = true;
            continue;
        }
        if best.map_or(true, |(current, _)| metric < current) {
            best = Some((metric, iface));
        }
    }
    (best.map(|(_, ip)| ip), tun_active)
}

#[cfg(windows)]
async fn inspect_default_routes() -> RouteInfo {
    let mut cmd = Command::new("route");
    cmd.args(["print", "-4"]);
    cmd.stdin(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    cmd.creation_flags(CREATE_NO_WINDOW);
    match time::timeout(Duration::from_secs(5), cmd.output()).await {
        Ok(Ok(output)) => {
            let (physical_ipv4, tun_active) =
                parse_route_print(&String::from_utf8_lossy(&output.stdout));
            RouteInfo {
                physical_ipv4,
                tun_active,
            }
        }
        _ => RouteInfo::default(),
    }
}

#[cfg(not(windows))]
async fn inspect_default_routes() -> RouteInfo {
    // Source-address binding does not bypass policy routing on Linux/macOS;
    // there we only switch to the real edge IPs.
    RouteInfo::default()
}

/// Resolve the real Cloudflare edge addresses through DNS-over-HTTPS so the
/// Fake-IP resolver is not involved.
async fn resolve_real_edge_ips() -> Vec<Ipv4Addr> {
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(4))
        .build()
    else {
        return Vec::new();
    };
    let mut found: Vec<Ipv4Addr> = Vec::new();
    for host in ["region1.v2.argotunnel.com", "region2.v2.argotunnel.com"] {
        for base in ["https://1.1.1.1/dns-query", "https://dns.google/resolve"] {
            let url = format!("{base}?name={host}&type=A");
            let Ok(response) = client
                .get(&url)
                .header("accept", "application/dns-json")
                .send()
                .await
            else {
                continue;
            };
            let Ok(body) = response.json::<serde_json::Value>().await else {
                continue;
            };
            let before = found.len();
            for ip in parse_doh_answers(&body).into_iter().take(6) {
                if !found.contains(&ip) {
                    found.push(ip);
                }
            }
            if found.len() > before {
                break;
            }
        }
    }
    found
}

pub(crate) fn parse_doh_answers(body: &serde_json::Value) -> Vec<Ipv4Addr> {
    body.get("Answer")
        .and_then(|answers| answers.as_array())
        .map(|answers| {
            answers
                .iter()
                .filter(|answer| answer.get("type").and_then(|t| t.as_u64()) == Some(1))
                .filter_map(|answer| answer.get("data").and_then(|d| d.as_str()))
                .filter_map(|data| data.trim().parse::<Ipv4Addr>().ok())
                .filter(|ip| !is_tun_like_ipv4(*ip))
                .collect()
        })
        .unwrap_or_default()
}

async fn plan_tun_bypass(physical_ipv4: Option<Ipv4Addr>) -> TunBypass {
    let mut ips = resolve_real_edge_ips().await;
    if ips.is_empty() {
        ips = FALLBACK_EDGE_IPS
            .iter()
            .filter_map(|ip| ip.parse().ok())
            .collect();
    }
    TunBypass {
        edges: ips
            .into_iter()
            .take(12)
            .map(|ip| format!("{ip}:7844"))
            .collect(),
        bind: physical_ipv4,
    }
}

fn append_log_line(log_path: &Path, line: &str) {
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    {
        let _ = writeln!(file, "{line}");
    }
}

struct AttemptConfig<'a> {
    cloudflared: &'a Path,
    cwd: &'a Path,
    log_path: &'a Path,
    quick: bool,
    port: u16,
    bind_address: &'a str,
    cloudflare_token: &'a str,
    named_public_url: &'a str,
    proxy: Option<&'a ProxyConfig>,
    protocol: Option<&'static str>,
}

/// Spawn one cloudflared process and wait for it to become ready. With
/// `strict`, only a registered edge connection counts as ready; on timeout or
/// exit the process is stopped and an error returned.
async fn spawn_attempt(
    config: &AttemptConfig<'_>,
    bypass: Option<&TunBypass>,
    timeout: Duration,
    strict: bool,
) -> AppResult<CloudflareTunnelHandle> {
    let quick = config.quick;
    let port = config.port;
    let mut cmd = Command::new(config.cloudflared);
    cmd.current_dir(config.cwd);
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

    if let Some(proxy) = config.proxy {
        apply_proxy_env(&mut cmd, proxy);
    }

    cmd.args(["tunnel", "--no-autoupdate"]);
    if let Some(protocol) = config.protocol {
        cmd.args(["--protocol", protocol]);
    }
    if let Some(bypass) = bypass {
        for edge in &bypass.edges {
            cmd.args(["--edge", edge]);
        }
        if let Some(bind) = bypass.bind {
            cmd.args(["--edge-bind-address", &bind.to_string()]);
            cmd.args(["--edge-ip-version", "4"]);
        }
    }
    if quick {
        let local_url = format!(
            "http://{}:{port}",
            crate::workspace::url_host_for_bind(config.bind_address)
        );
        cmd.args(["--url", &local_url]);
    } else {
        cmd.args(["run", "--token", config.cloudflare_token.trim()]);
    }

    let mut child = cmd
        .spawn()
        .map_err(|err| AppError::Message(format!("启动 cloudflared 失败: {err}")))?;
    let pid = child.id();

    let (ready_tx, ready_rx) = oneshot::channel();
    let log_path = config.log_path.to_path_buf();
    let named_url = config.named_public_url.trim_end_matches('/').to_string();
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
                strict,
                named_url,
                seen_url_for_stream,
                ready_tx,
            )
            .await;
        });
    } else {
        let _ = ready_tx.send(QuickTunnelReady {
            public_url: if quick { None } else { Some(named_url) },
            registered: false,
            exited: false,
        });
    }

    let outcome = time::timeout(timeout, ready_rx).await;
    let failure = match outcome {
        Ok(Ok(ready)) if ready.registered || (!ready.exited && !strict) => {
            let public_url = if quick {
                ready.public_url
            } else {
                Some(config.named_public_url.trim_end_matches('/').to_string())
            };
            match public_url {
                Some(public_url) => {
                    return Ok(CloudflareTunnelHandle {
                        child,
                        public_url,
                        pid,
                    })
                }
                None => format!(
                    "cloudflared 已启动，但没有解析到 trycloudflare.com 地址。请查看日志：{}",
                    log_path_for_error.display()
                ),
            }
        }
        Ok(Ok(_)) | Ok(Err(_)) => format!(
            "cloudflared 已退出，未能连上 Cloudflare。请查看日志：{}",
            log_path_for_error.display()
        ),
        Err(_) => {
            // The quick-tunnel URL is allocated before the edge connection is
            // registered. Outside strict mode, hand it out: cloudflared keeps
            // retrying in the background and the supervisor reports health.
            let fallback = if quick && !strict {
                seen_url.lock().ok().and_then(|guard| guard.clone())
            } else {
                None
            };
            if let Some(public_url) = fallback {
                return Ok(CloudflareTunnelHandle {
                    child,
                    public_url,
                    pid,
                });
            }
            format!(
                "cloudflared 已启动，但在 {} 秒内没有连上 Cloudflare。\n\
                 请检查：1) MCP 服务是否已在本机端口 {port} 运行；2) 设置 → 通用 → 网络代理 是否配置正确（如 http://127.0.0.1:7890）；\
                 3) 查看日志 {}",
                timeout.as_secs(),
                log_path_for_error.display()
            )
        }
    };

    let _ = stop_child(child, pid).await;
    Err(AppError::Message(failure))
}

struct QuickTunnelReady {
    public_url: Option<String>,
    /// An edge connection was registered.
    registered: bool,
    /// The output stream ended (cloudflared exited) before readiness.
    exited: bool,
}

async fn stream_cloudflare_output<R, E>(
    stdout: R,
    stderr: Option<E>,
    log_path: &Path,
    quick: bool,
    strict: bool,
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
                    registered: false,
                    exited: false,
                });
            }
            return;
        }
    };

    let send_ready = |tx: &mut Option<oneshot::Sender<QuickTunnelReady>>,
                      url: Option<String>,
                      registered: bool,
                      exited: bool| {
        if let Some(sender) = tx.take() {
            let _ = sender.send(QuickTunnelReady {
                public_url: url,
                registered,
                exited,
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
                    send_ready(ready_tx, public_url.clone(), true, false);
                }
            } else {
                let lowered = line.to_ascii_lowercase();
                if lowered.contains("registered tunnel connection") {
                    send_ready(ready_tx, Some(named_url.clone()), true, false);
                } else if !strict && lowered.contains("starting metrics server") {
                    send_ready(ready_tx, Some(named_url.clone()), false, false);
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

    send_ready(&mut ready_tx, public_url, false, true);
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
    use super::{
        extract_trycloudflare_url, is_fake_ip, is_tun_like_ipv4, parse_doh_answers,
        parse_route_print, resolve_tunnel_protocol,
    };
    use std::net::Ipv4Addr;

    #[test]
    fn route_print_finds_physical_uplink_under_flclash_tun() {
        let text = "\
===========================================================================
IPv4 Route Table
===========================================================================
Active Routes:
Network Destination        Netmask          Gateway       Interface  Metric
          0.0.0.0          0.0.0.0      192.168.1.1    192.168.1.23     35
          0.0.0.0          0.0.0.0       172.19.0.2      172.19.0.1      0
          0.0.0.0        128.0.0.0          On-link      172.19.0.1      0
        127.0.0.0        255.0.0.0         在链路上       127.0.0.1    331
      192.168.1.0    255.255.255.0          On-link    192.168.1.23    291
===========================================================================
Persistent Routes:
  Network Address          Netmask  Gateway Address  Metric
          0.0.0.0          0.0.0.0      192.168.1.1  Default
===========================================================================
";
        let (physical, tun) = parse_route_print(text);
        assert_eq!(physical, Some(Ipv4Addr::new(192, 168, 1, 23)));
        assert!(tun);
    }

    #[test]
    fn route_print_without_tun() {
        let text = "          0.0.0.0          0.0.0.0     10.0.0.1     10.0.0.50     25\n          0.0.0.0          0.0.0.0     10.0.1.1     10.0.1.50     50\n";
        let (physical, tun) = parse_route_print(text);
        assert_eq!(physical, Some(Ipv4Addr::new(10, 0, 0, 50)));
        assert!(!tun);
        assert!(is_tun_like_ipv4(Ipv4Addr::new(198, 18, 0, 1)));
        assert!(!is_tun_like_ipv4(Ipv4Addr::new(192, 168, 1, 2)));
    }

    #[test]
    fn parses_doh_json_and_drops_fake_ips() {
        let body = serde_json::json!({
            "Status": 0,
            "Answer": [
                {"name": "region1.v2.argotunnel.com", "type": 1, "data": "198.41.192.7"},
                {"name": "region1.v2.argotunnel.com", "type": 1, "data": "198.18.0.188"},
                {"name": "x", "type": 5, "data": "cname.example."}
            ]
        });
        assert_eq!(
            parse_doh_answers(&body),
            vec![Ipv4Addr::new(198, 41, 192, 7)]
        );
        assert!(parse_doh_answers(&serde_json::json!({"Status": 3})).is_empty());
    }

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

/// Accepts whatever the user copied from the Cloudflare dashboard and returns
/// just the tunnel token. Handles e.g.
/// `cloudflared.exe service install eyJ...`, `sudo cloudflared service install eyJ...`,
/// `cloudflared tunnel run --token eyJ...` and `--token=eyJ...`, with or without quotes.
pub fn normalize_cloudflare_token(raw: &str) -> String {
    fn unquote(word: &str) -> &str {
        word.trim_matches(|c| c == '"' || c == '\'' || c == '`')
    }
    let trimmed = raw.trim();
    let words: Vec<&str> = trimmed
        .split_whitespace()
        .map(unquote)
        .filter(|word| !word.is_empty())
        .collect();
    if words.len() <= 1 {
        return words
            .first()
            .map(|word| {
                word.strip_prefix("--token=")
                    .map_or(*word, unquote)
                    .to_string()
            })
            .unwrap_or_default();
    }
    for (index, word) in words.iter().enumerate() {
        if let Some(value) = word.strip_prefix("--token=") {
            return unquote(value).to_string();
        }
        if *word == "--token" {
            if let Some(next) = words.get(index + 1) {
                return next.to_string();
            }
        }
    }
    if let Some(token) = words
        .iter()
        .rev()
        .find(|word| looks_like_tunnel_token(word))
    {
        return token.to_string();
    }
    trimmed.to_string()
}

fn looks_like_tunnel_token(word: &str) -> bool {
    word.len() >= 40
        && word.starts_with("eyJ")
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '-' | '_'))
}

/// Decodes a tunnel token (base64 JSON `{"a": account, "t": tunnel id, "s": secret}`)
/// and returns the tunnel id, or `None` when it is not a valid tunnel token.
pub fn cloudflare_token_tunnel_id(token: &str) -> Option<String> {
    use base64::Engine;
    let token = normalize_cloudflare_token(token);
    let trimmed = token.trim_end_matches('=');
    let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(trimmed)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(trimmed))
        .ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let tunnel = value.get("t")?.as_str()?;
    (value.get("a").is_some() && value.get("s").is_some() && !tunnel.is_empty())
        .then(|| tunnel.to_string())
}

#[cfg(test)]
mod token_and_version_tests {
    use super::*;

    const TOKEN: &str = "eyJhIjoiMTIzNDU2Nzg5MGFiY2RlZiIsInQiOiI1ZjNlYjZlOC0xMjM0LTQ1NjctODlhYi1jZGVmMDEyMzQ1NjciLCJzIjoiYzJWamNtVjAifQ==";

    #[test]
    fn strips_dashboard_install_commands() {
        for raw in [
            TOKEN.to_string(),
            format!("  {TOKEN}\n"),
            format!("cloudflared.exe service install {TOKEN}"),
            format!("sudo cloudflared service install {TOKEN}"),
            format!("cloudflared tunnel run --token {TOKEN}"),
            format!("cloudflared tunnel --no-autoupdate run --token=\"{TOKEN}\""),
            format!("--token={TOKEN}"),
            format!(
                "& 'C:\\Program Files\\cloudflared\\cloudflared.exe' service install '{TOKEN}'"
            ),
        ] {
            assert_eq!(normalize_cloudflare_token(&raw), TOKEN, "input: {raw}");
        }
        assert_eq!(normalize_cloudflare_token(""), "");
    }

    #[test]
    fn decodes_tunnel_id() {
        assert_eq!(
            cloudflare_token_tunnel_id(&format!("cloudflared.exe service install {TOKEN}"))
                .as_deref(),
            Some("5f3eb6e8-1234-4567-89ab-cdef01234567")
        );
        assert_eq!(cloudflare_token_tunnel_id("not-a-token"), None);
    }

    #[test]
    fn parses_versions() {
        assert_eq!(
            parse_cloudflared_version("cloudflared version 2025.6.1 (built 2025-06-16-1234 UTC)"),
            Some((2025, 6, 1))
        );
        assert_eq!(parse_cloudflared_version("2026.9.3"), Some((2026, 9, 3)));
        assert_eq!(parse_cloudflared_version("garbage"), None);
        assert!(parse_cloudflared_version("2025.6.1").unwrap() < pinned_cloudflared_version());
        assert!((2026, 10, 0) > pinned_cloudflared_version());
    }
}
