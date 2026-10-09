//! A packaged newer executable can replace the older desktop holding the mutex.
//! The detached worker rechecks process identity and restores saved services.
use std::os::windows::process::CommandExt;

pub fn schedule() -> Result<(), String> {
    if std::env::args().any(|arg| arg == "--handoff-child") {
        return Err("Replacement could not acquire the single-instance lock".into());
    }
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let config = crate::platform::platform()
        .app_config_dir()
        .map_err(|e| e.to_string())?;
    let workers = config.join("handoff-workers");
    std::fs::create_dir_all(&workers).map_err(|e| e.to_string())?;
    let script = workers.join(format!("startup-{}.ps1", uuid::Uuid::new_v4()));
    std::fs::write(
        &script,
        include_str!("../../scripts/desktop-startup-handoff.ps1"),
    )
    .map_err(|e| e.to_string())?;
    let powershell =
        std::path::PathBuf::from(std::env::var_os("SystemRoot").ok_or("SystemRoot is missing")?)
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let result = std::process::Command::new(powershell)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script)
        .arg("-TemporaryWorker")
        .arg("-NewExe")
        .arg(executable)
        .args([
            "-NewVersion",
            env!("CARGO_PKG_VERSION"),
            "-LauncherPid",
            &std::process::id().to_string(),
        ])
        .arg("-DataFile")
        .arg(config.join("data/profiles.json"))
        .creation_flags(0x08000000) // CREATE_NO_WINDOW: update diagnostics use a dialog/log.
        .spawn();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&script);
        return Err(error.to_string());
    }
    Ok(())
}
