use std::path::Path;
use std::process::Command;

use crate::error::{AppError, AppResult};

pub fn open_path_in_file_manager(path: &Path) -> AppResult<()> {
    if !path.is_dir() {
        return Err(AppError::Message(format!(
            "路径不存在或不是目录: {}",
            path.display()
        )));
    }

    #[cfg(target_os = "windows")]
    {
        Command::new("explorer")
            .arg(path)
            .spawn()
            .map_err(|err| AppError::Message(format!("无法打开目录: {err}")))
            .map(|_| ())
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(path)
            .spawn()
            .map_err(|err| AppError::Message(format!("无法打开目录: {err}")))
            .map(|_| ())
    }

    #[cfg(target_os = "linux")]
    {
        Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map_err(|err| AppError::Message(format!("无法打开目录: {err}")))
            .map(|_| ())
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        let _ = path;
        Err(AppError::Message("当前平台不支持打开目录。".into()))
    }
}

/// Validates that `raw` is an absolute http(s) URL without embedded credentials
/// and returns its normalized form.
pub fn validate_external_url(raw: &str) -> AppResult<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > 8192 || trimmed.chars().any(char::is_control) {
        return Err(AppError::Message("链接无效".into()));
    }
    let url = reqwest::Url::parse(trimmed).map_err(|_| AppError::Message("链接无效".into()))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().map_or(true, str::is_empty) {
        return Err(AppError::Message("只支持打开 http/https 链接".into()));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AppError::Message("链接不能包含账号或密码".into()));
    }
    Ok(url.to_string())
}

/// Opens an http(s) URL in the user's default browser (downloads are handled
/// by the browser). The URL is passed as a single argument, never through a
/// shell, so `&`, `|` and quotes in query strings are not interpreted.
pub fn open_url_in_browser(raw: &str) -> AppResult<()> {
    let url = validate_external_url(raw)?;
    let spawn = |program: &str, args: &[&str]| {
        Command::new(program)
            .args(args)
            .spawn()
            .map(|_| ())
            .map_err(|err| AppError::Message(format!("无法打开浏览器: {err}")))
    };
    #[cfg(target_os = "windows")]
    {
        spawn("rundll32.exe", &["url.dll,FileProtocolHandler", &url])
    }
    #[cfg(target_os = "macos")]
    {
        spawn("open", &[&url])
    }
    #[cfg(target_os = "linux")]
    {
        spawn("xdg-open", &[&url])
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        let _ = (url, spawn);
        Err(AppError::Message("当前平台不支持打开链接。".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::validate_external_url;

    #[test]
    fn external_urls_are_limited_to_http_and_https() {
        assert_eq!(
            validate_external_url(" https://github.com/a/b/releases/download/v1/x.exe?a=1&b=2 ").unwrap(),
            "https://github.com/a/b/releases/download/v1/x.exe?a=1&b=2"
        );
        assert!(validate_external_url("http://127.0.0.1:8080/").is_ok());
        for bad in [
            "",
            "file:///C:/Windows/System32/calc.exe",
            "javascript:alert(1)",
            "ms-settings:privacy",
            "https://user:pass@example.com/",
            "https://example.com/\nfoo",
            "C:\\Windows\\notepad.exe",
            "https://",
        ] {
            assert!(validate_external_url(bad).is_err(), "{bad}");
        }
    }
}
