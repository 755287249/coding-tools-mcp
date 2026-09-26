//! Software management for tunnel clients and sandbox CLIs.
//!
//! Tunnel binaries (`frpc`, `cloudflared`) can be installed into the app cache
//! `bin/` directory (downloaded from GitHub, honoring the mirror + proxy config).
//! Cache copies are "managed" (uninstallable); binaries found on PATH or in
//! system install locations are reported but cannot be removed from here.
//!
//! Sandbox CLIs (`sbx`, `wslc`, `docker`, `podman`) are installed through
//! official package managers (WinGet / MSI / `wsl --update` / Homebrew).
//! Login and engine start stay user-run; the app never launches `sbx login`,
//! Docker Desktop, or `podman machine`.

use std::path::PathBuf;
#[cfg(any(windows, target_os = "macos"))]
use std::process::Stdio;
#[cfg(any(windows, target_os = "macos"))]
use std::time::Duration;

use serde::Serialize;
#[cfg(windows)]
use sha2::{Digest, Sha256};

use crate::error::{AppError, AppResult};
use crate::platform::platform;
use crate::tools::sandbox::{
    discovered_docker_program, discovered_podman_program, discovered_sbx_program,
    discovered_wslc_program,
};
use crate::tunnel::cloudflare::resolve_cloudflared;
use crate::tunnel::cloudflare::{
    cached_cloudflared_path, cached_cloudflared_state, download_cloudflared_to_cache,
};
use crate::tunnel::frp::{cached_frpc_path, download_frpc_to_cache, resolve_frpc};

#[cfg(windows)]
const SBX_VERSION: &str = "0.38.0";
#[cfg(windows)]
const SBX_MSI_SHA256: &str = "4450c8a1782787683f90902d10b2335ef03436cf3ac75671d064110a64f1eff1";
#[cfg(windows)]
const SBX_WINGET_ID: &str = "Docker.sbx";
#[cfg(windows)]
const DOCKER_WINGET_ID: &str = "Docker.DockerDesktop";
#[cfg(windows)]
const PODMAN_WINGET_ID: &str = "RedHat.Podman";

const SBX_LOGIN_GUIDANCE: &str = "安裝完成後請在終端機自行登入，應用程式不會代登：\n1. sbx login\n2. sbx policy init deny-all\n也可改用 balanced 或 allow-all。";
const DOCKER_START_GUIDANCE: &str = "安裝完成後請自行啟動 Docker Desktop，並確認 `docker info` 成功。應用程式不會代為啟動引擎或登入。";
const PODMAN_MACHINE_GUIDANCE: &str = "安裝完成後請在終端機自行建立並啟動機器，應用程式不會代跑：\n1. podman machine init\n2. podman machine start";

/// Status of a managed or detected host binary, serialized to the frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SoftwareStatus {
    /// "frpc" | "cloudflared" | "sbx" | "wslc" | "docker" | "podman"
    pub kind: String,
    /// Human-facing display name.
    pub name: String,
    /// Whether the binary was found anywhere (cache, PATH, or system dir).
    pub installed: bool,
    /// Resolved path if found.
    pub path: String,
    /// True when the resolved binary lives in the app cache dir (uninstallable).
    pub managed: bool,
    /// "tunnel" or "sandbox".
    pub group: String,
    /// Whether this host can install the missing tool from Software management.
    pub installable: bool,
    /// Extra install/use guidance shown in the UI.
    pub hint: String,
    /// User-run follow-up steps after a successful install. Never executed by the app.
    pub next_steps: String,
    /// Known version of the resolved binary (empty when unknown).
    pub version: String,
    /// True when the app-managed copy is older than the pinned release and
    /// should be updated before use (Cloudflare rejects year-old connectors).
    pub outdated: bool,
}

fn frpc_status() -> SoftwareStatus {
    let cache = cached_frpc_path().filter(|p| p.is_file());
    let resolved = resolve_frpc().ok();
    // Prefer showing the cache-managed copy when present.
    let (path, managed, installed) = match (&cache, &resolved) {
        (Some(cache_path), _) => (cache_path.clone(), true, true),
        (None, Some(found)) => (found.clone(), false, true),
        (None, None) => (PathBuf::new(), false, false),
    };
    SoftwareStatus {
        kind: "frpc".into(),
        name: "frp 客户端 (frpc)".into(),
        installed,
        path: path.to_string_lossy().to_string(),
        managed,
        group: "tunnel".into(),
        installable: !installed || managed,
        hint: "Built-in WSS 以外的 FRP 通道需要此用戶端。".into(),
        version: String::new(),
        outdated: false,
        next_steps: String::new(),
    }
}

fn cloudflared_status() -> SoftwareStatus {
    let cache = cached_cloudflared_path().filter(|p| p.is_file());
    let resolved = resolve_cloudflared().ok();
    let (path, managed, installed) = match (&cache, &resolved) {
        (Some(cache_path), _) => (cache_path.clone(), true, true),
        (None, Some(found)) => (found.clone(), false, true),
        (None, None) => (PathBuf::new(), false, false),
    };
    let (version, outdated) = if managed {
        cached_cloudflared_state(&path)
    } else {
        (String::new(), false)
    };
    SoftwareStatus {
        kind: "cloudflared".into(),
        name: "Cloudflare Tunnel (cloudflared)".into(),
        installed,
        path: path.to_string_lossy().to_string(),
        managed,
        group: "tunnel".into(),
        installable: !installed || managed,
        hint: "Cloudflare Quick / Named Tunnel 需要此用戶端。".into(),
        version,
        outdated,
        next_steps: String::new(),
    }
}

fn sbx_status() -> SoftwareStatus {
    let resolved = discovered_sbx_program();
    let installed = resolved.as_ref().is_some_and(|path| path.is_file());
    SoftwareStatus {
        kind: "sbx".into(),
        name: "Docker Sandboxes (sbx)".into(),
        installed,
        path: resolved
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default(),
        managed: false,
        group: "sandbox".into(),
        installable: cfg!(windows) && !installed,
        hint: if installed {
            SBX_LOGIN_GUIDANCE.into()
        } else if cfg!(windows) {
            "按安裝會自動用 WinGet 或官方 MSI 安裝。登入請在安裝完成後自行執行 sbx login。".into()
        } else if cfg!(target_os = "macos") {
            "macOS 請先安裝 Homebrew，再按安裝自動執行 brew install docker/tap/sbx。登入請之後自行執行 sbx login。".into()
        } else {
            "Linux 請用官方套件管理員安裝 docker-sbx。登入請之後自行執行 sbx login。".into()
        },
        version: String::new(),
        outdated: false,
        next_steps: if installed {
            SBX_LOGIN_GUIDANCE.into()
        } else {
            String::new()
        },
    }
}

fn engine_installable(installed: bool) -> bool {
    (cfg!(windows) || cfg!(target_os = "macos")) && !installed
}

fn docker_status() -> SoftwareStatus {
    let resolved = discovered_docker_program();
    let installed = resolved.as_ref().is_some_and(|path| path.is_file());
    SoftwareStatus {
        kind: "docker".into(),
        name: "Docker".into(),
        installed,
        path: resolved
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default(),
        managed: false,
        group: "sandbox".into(),
        installable: engine_installable(installed),
        hint: if installed {
            DOCKER_START_GUIDANCE.into()
        } else if cfg!(windows) {
            "命令沙盒的原生 Docker 後端。按安裝會自動用 WinGet 安裝 Docker Desktop。引擎請之後自行啟動。".into()
        } else if cfg!(target_os = "macos") {
            "命令沙盒的原生 Docker 後端。按安裝會自動執行 brew install --cask docker。引擎請之後自行啟動。".into()
        } else {
            "Linux 請用發行版套件管理員安裝 Docker Engine。".into()
        },
        version: String::new(),
        outdated: false,
        next_steps: if installed {
            DOCKER_START_GUIDANCE.into()
        } else {
            String::new()
        },
    }
}

fn podman_status() -> SoftwareStatus {
    let resolved = discovered_podman_program();
    let installed = resolved.as_ref().is_some_and(|path| path.is_file());
    SoftwareStatus {
        kind: "podman".into(),
        name: "Podman".into(),
        installed,
        path: resolved
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default(),
        managed: false,
        group: "sandbox".into(),
        installable: engine_installable(installed),
        hint: if installed {
            PODMAN_MACHINE_GUIDANCE.into()
        } else if cfg!(windows) {
            "命令沙盒的原生 Podman 後端。按安裝會自動用 WinGet 安裝 Podman。機器請之後自行 init/start。".into()
        } else if cfg!(target_os = "macos") {
            "命令沙盒的原生 Podman 後端。按安裝會自動執行 brew install podman。機器請之後自行 init/start。".into()
        } else {
            "Linux 請用發行版套件管理員安裝 Podman。".into()
        },
        version: String::new(),
        outdated: false,
        next_steps: if installed {
            PODMAN_MACHINE_GUIDANCE.into()
        } else {
            String::new()
        },
    }
}

fn wslc_status() -> SoftwareStatus {
    let resolved = discovered_wslc_program();
    let installed = resolved.as_ref().is_some_and(|path| path.is_file());
    SoftwareStatus {
        kind: "wslc".into(),
        name: "Microsoft WSL Containers (wslc)".into(),
        installed,
        path: resolved
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default(),
        managed: false,
        group: "sandbox".into(),
        installable: cfg!(windows) && !installed,
        hint: if cfg!(windows) {
            "命令沙盒的 WSL Containers 後端。按安裝會自動執行 wsl --update。".into()
        } else {
            "WSL Containers 只在 Windows 上提供。".into()
        },
        version: String::new(),
        outdated: false,
        next_steps: String::new(),
    }
}

/// Report install status for tunnel and sandbox binaries.
pub fn list_software() -> Vec<SoftwareStatus> {
    vec![
        frpc_status(),
        cloudflared_status(),
        sbx_status(),
        wslc_status(),
        docker_status(),
        podman_status(),
    ]
}

/// Install the requested binary.
pub async fn install_software(kind: &str) -> AppResult<SoftwareStatus> {
    match kind {
        "frpc" => {
            download_frpc_to_cache().await?;
            Ok(frpc_status())
        }
        "cloudflared" => {
            download_cloudflared_to_cache().await?;
            Ok(cloudflared_status())
        }
        "sbx" => install_sbx().await,
        "wslc" => install_wslc().await,
        "docker" => install_docker().await,
        "podman" => install_podman().await,
        other => Err(AppError::Message(format!("未知软件: {other}"))),
    }
}

/// Uninstall a cache-managed binary. Refuses if the binary is not in the cache
/// dir (i.e. it was installed by the system / winget / apt and is not ours).
pub fn uninstall_software(kind: &str) -> AppResult<SoftwareStatus> {
    let cache_path = match kind {
        "frpc" => cached_frpc_path(),
        "cloudflared" => cached_cloudflared_path(),
        "sbx" | "wslc" | "docker" | "podman" => {
            return Err(AppError::Message(
                "該沙盒工具是系統／套件管理員安裝的，無法在此卸載。".into(),
            ));
        }
        other => return Err(AppError::Message(format!("未知软件: {other}"))),
    };

    let Some(path) = cache_path else {
        return Err(AppError::Message("无法解析缓存目录。".into()));
    };

    if path.is_file() {
        std::fs::remove_file(&path)?;
    } else {
        return Err(AppError::Message(
            "该软件不是由本应用安装的，无法在此卸载。".into(),
        ));
    }

    // Also clear any cached download archives for frpc to force a fresh fetch.
    if kind == "frpc" {
        if let Ok(dir) = platform().app_config_dir() {
            let downloads = dir.join("bin").join("downloads");
            let _ = std::fs::remove_dir_all(&downloads);
        }
    }

    Ok(match kind {
        "frpc" => frpc_status(),
        _ => cloudflared_status(),
    })
}

async fn install_sbx() -> AppResult<SoftwareStatus> {
    if sbx_status().installed {
        return Ok(sbx_status());
    }
    #[cfg(windows)]
    {
        if let Err(winget_error) = install_with_winget(SBX_WINGET_ID).await {
            install_sbx_with_msi().await.map_err(|msi_error| {
                AppError::Message(format!(
                    "{winget_error}；接著嘗試官方 MSI 也失敗：{msi_error}"
                ))
            })?;
        }
        let status = sbx_status();
        if status.installed {
            return Ok(status);
        }
        return Err(AppError::Message(
            "已執行 Docker Sandboxes 安裝，但找不到 sbx.exe。請確認安裝完成後重新開啟應用程式。"
                .into(),
        ));
    }
    #[cfg(target_os = "macos")]
    {
        install_sbx_with_brew().await?;
        let status = sbx_status();
        if status.installed {
            return Ok(status);
        }
        return Err(AppError::Message(
            "已執行 Homebrew 安裝，但找不到 sbx。請確認 brew 安裝完成後重新開啟應用程式。".into(),
        ));
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Err(AppError::Message(
            "此平台請用官方套件管理員安裝 Docker Sandboxes：sudo apt-get install docker-sbx。"
                .into(),
        ))
    }
}

async fn install_docker() -> AppResult<SoftwareStatus> {
    if docker_status().installed {
        return Ok(docker_status());
    }
    #[cfg(windows)]
    {
        install_with_winget(DOCKER_WINGET_ID)
            .await
            .map_err(AppError::Message)?;
        let status = docker_status();
        if status.installed {
            return Ok(status);
        }
        return Err(AppError::Message(
            "已執行 Docker Desktop 安裝，但找不到 docker.exe。請確認安裝完成後重新開啟應用程式，並自行啟動 Docker Desktop。"
                .into(),
        ));
    }
    #[cfg(target_os = "macos")]
    {
        install_with_brew(&["install", "--cask", "docker"]).await?;
        let status = docker_status();
        if status.installed {
            return Ok(status);
        }
        return Err(AppError::Message(
            "已執行 Homebrew 安裝，但找不到 docker。請確認 Docker Desktop 安裝完成後重新開啟應用程式。".into(),
        ));
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Err(AppError::Message(
            "此平台請用發行版套件管理員安裝 Docker Engine。".into(),
        ))
    }
}

async fn install_podman() -> AppResult<SoftwareStatus> {
    if podman_status().installed {
        return Ok(podman_status());
    }
    #[cfg(windows)]
    {
        install_with_winget(PODMAN_WINGET_ID)
            .await
            .map_err(AppError::Message)?;
        let status = podman_status();
        if status.installed {
            return Ok(status);
        }
        return Err(AppError::Message(
            "已執行 Podman 安裝，但找不到 podman.exe。請確認安裝完成後重新開啟應用程式，並自行執行 podman machine init / start。"
                .into(),
        ));
    }
    #[cfg(target_os = "macos")]
    {
        install_with_brew(&["install", "podman"]).await?;
        let status = podman_status();
        if status.installed {
            return Ok(status);
        }
        return Err(AppError::Message(
            "已執行 Homebrew 安裝，但找不到 podman。請確認安裝完成後重新開啟應用程式。".into(),
        ));
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Err(AppError::Message(
            "此平台請用發行版套件管理員安裝 Podman。".into(),
        ))
    }
}

async fn install_wslc() -> AppResult<SoftwareStatus> {
    if wslc_status().installed {
        return Ok(wslc_status());
    }
    #[cfg(windows)]
    {
        let output = run_host_command("wsl.exe", &["--update"], Duration::from_secs(600))
            .await
            .map_err(|error| {
                AppError::Message(format!(
                    "無法更新 WSL 以安裝 WSL Containers：{error}。請先確認已安裝 WSL，或以系統管理員執行 wsl --update。"
                ))
            })?;
        if !output.status.success() {
            return Err(AppError::Message(format!(
                "wsl --update 失敗（exit {}）。stdout={} stderr={}",
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        let status = wslc_status();
        if status.installed {
            return Ok(status);
        }
        return Err(AppError::Message(
            "WSL 已更新，但仍找不到 wslc.exe。請重開應用程式，或確認 WSL 版本已包含 WSL Containers。"
                .into(),
        ));
    }
    #[cfg(not(windows))]
    {
        Err(AppError::Message(
            "WSL Containers 只在 Windows 上提供。".into(),
        ))
    }
}

#[cfg(windows)]
async fn install_with_winget(package_id: &str) -> Result<(), String> {
    let program = winget_program().ok_or_else(|| "找不到 winget".to_string())?;
    let output = run_host_command(
        &program,
        &[
            "install",
            "--id",
            package_id,
            "--exact",
            "--silent",
            "--accept-package-agreements",
            "--accept-source-agreements",
            "--disable-interactivity",
        ],
        Duration::from_secs(900),
    )
    .await
    .map_err(|error| error.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "winget install {package_id} 失敗（exit {}）：{}",
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stderr)
    ))
}

#[cfg(windows)]
async fn install_sbx_with_msi() -> AppResult<()> {
    let settings = crate::settings::AppSettings::load_or_default();
    let url = format!(
        "https://github.com/docker/sbx-releases/releases/download/v{SBX_VERSION}/DockerSandboxes.msi"
    );
    let cache_dir = platform().app_config_dir()?.join("bin").join("downloads");
    std::fs::create_dir_all(&cache_dir)?;
    let msi_path = cache_dir.join(format!("DockerSandboxes-{SBX_VERSION}.msi"));
    let bytes = crate::tunnel::download::download_release_asset(&settings, &url, "sbx").await?;
    verify_sha256(&bytes, SBX_MSI_SHA256, "sbx")?;
    std::fs::write(&msi_path, &bytes)?;
    let output = run_host_command(
        "msiexec.exe",
        &["/i", &msi_path.to_string_lossy(), "/qn", "/norestart"],
        Duration::from_secs(600),
    )
    .await?;
    if output.status.success() {
        return Ok(());
    }
    Err(AppError::Message(format!(
        "msiexec 安裝 Docker Sandboxes 失敗（exit {}）：{}",
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stderr)
    )))
}

#[cfg(target_os = "macos")]
async fn install_sbx_with_brew() -> AppResult<()> {
    let brew = which::which("brew").map_err(|_| {
        AppError::Message("找不到 Homebrew。請先安裝 brew，再回來自動安裝 sbx。".into())
    })?;
    let tap = run_host_command(
        &brew.to_string_lossy(),
        &["trust", "docker/tap"],
        Duration::from_secs(120),
    )
    .await?;
    if !tap.status.success() {
        return Err(AppError::Message(format!(
            "brew trust docker/tap 失敗：{}",
            String::from_utf8_lossy(&tap.stderr)
        )));
    }
    install_with_brew(&["install", "docker/tap/sbx"]).await
}

#[cfg(target_os = "macos")]
async fn install_with_brew(args: &[&str]) -> AppResult<()> {
    let brew = which::which("brew").map_err(|_| {
        AppError::Message("找不到 Homebrew。請先安裝 brew，再回來自動安裝。".into())
    })?;
    let install = run_host_command(&brew.to_string_lossy(), args, Duration::from_secs(900)).await?;
    if install.status.success() {
        return Ok(());
    }
    Err(AppError::Message(format!(
        "brew {} 失敗：{}",
        args.join(" "),
        String::from_utf8_lossy(&install.stderr)
    )))
}

#[cfg(windows)]
fn winget_program() -> Option<String> {
    if let Ok(path) = which::which("winget") {
        return Some(path.to_string_lossy().into_owned());
    }
    let local_app_data = std::env::var_os("LOCALAPPDATA")?;
    let candidate = PathBuf::from(local_app_data)
        .join("Microsoft")
        .join("WindowsApps")
        .join("winget.exe");
    candidate
        .is_file()
        .then_some(candidate.to_string_lossy().into_owned())
}

#[cfg(any(windows, target_os = "macos"))]
async fn run_host_command(
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> AppResult<std::process::Output> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let child = command
        .spawn()
        .map_err(|error| AppError::Message(format!("無法啟動 {program}：{error}")))?;
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => Err(AppError::Message(format!("執行 {program} 失敗：{error}"))),
        Err(_) => Err(AppError::Message(format!(
            "執行 {program} 超過 {} 秒。",
            timeout.as_secs()
        ))),
    }
}

#[cfg(windows)]
fn verify_sha256(bytes: &[u8], expected: &str, label: &str) -> AppResult<()> {
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(AppError::Message(format!(
            "{label} 安裝包 SHA-256 驗證失敗，預期 {expected}，實際 {actual}。已拒絕安裝。"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_software_includes_tunnel_and_sandbox_tools() {
        let items = list_software();
        let kinds = items
            .iter()
            .map(|item| item.kind.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            ["frpc", "cloudflared", "sbx", "wslc", "docker", "podman"]
        );
        let sbx = items.iter().find(|item| item.kind == "sbx").expect("sbx");
        assert_eq!(sbx.group, "sandbox");
        assert_eq!(sbx.managed, false);
        assert_eq!(sbx.installable, cfg!(windows) && !sbx.installed);
        assert!(
            sbx.next_steps.is_empty() || sbx.next_steps.contains("sbx login"),
            "installed sbx should only guide login, never claim the app will log in"
        );
        let wslc = items.iter().find(|item| item.kind == "wslc").expect("wslc");
        assert_eq!(wslc.group, "sandbox");
        assert_eq!(wslc.installable, cfg!(windows) && !wslc.installed);
        let docker = items
            .iter()
            .find(|item| item.kind == "docker")
            .expect("docker");
        assert_eq!(docker.group, "sandbox");
        assert_eq!(
            docker.installable,
            (cfg!(windows) || cfg!(target_os = "macos")) && !docker.installed
        );
        let podman = items
            .iter()
            .find(|item| item.kind == "podman")
            .expect("podman");
        assert_eq!(podman.group, "sandbox");
        assert_eq!(
            podman.installable,
            (cfg!(windows) || cfg!(target_os = "macos")) && !podman.installed
        );
    }

    #[test]
    fn sandbox_tools_cannot_be_uninstalled_from_the_app_cache() {
        let error = uninstall_software("sbx").expect_err("sbx is not cache-managed");
        assert!(error.to_string().contains("無法在此卸載"));
        let error = uninstall_software("wslc").expect_err("wslc is not cache-managed");
        assert!(error.to_string().contains("無法在此卸載"));
        let error = uninstall_software("docker").expect_err("docker is not cache-managed");
        assert!(error.to_string().contains("無法在此卸載"));
        let error = uninstall_software("podman").expect_err("podman is not cache-managed");
        assert!(error.to_string().contains("無法在此卸載"));
    }

    #[test]
    fn sbx_login_guidance_is_user_run_only() {
        assert!(SBX_LOGIN_GUIDANCE.contains("sbx login"));
        assert!(SBX_LOGIN_GUIDANCE.contains("不會代登"));
        assert!(!SBX_LOGIN_GUIDANCE.contains("winget"));
    }

    #[test]
    fn docker_and_podman_guidance_does_not_start_the_engine() {
        assert!(DOCKER_START_GUIDANCE.contains("自行啟動"));
        assert!(DOCKER_START_GUIDANCE.contains("不會代為啟動"));
        assert!(PODMAN_MACHINE_GUIDANCE.contains("podman machine init"));
        assert!(PODMAN_MACHINE_GUIDANCE.contains("不會代跑"));
    }

    #[tokio::test]
    async fn unknown_software_kind_fails_closed() {
        let error = install_software("not-a-real-tool")
            .await
            .expect_err("unknown kind");
        assert!(error.to_string().contains("未知软件"));
    }
}
