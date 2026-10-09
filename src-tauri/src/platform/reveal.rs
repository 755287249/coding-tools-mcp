use std::{fs, path::{Path, PathBuf}, process::Command};

/// Explicit local UI click only. Relative paths remain inside the selected root.
pub(crate) fn resolve_chat_path(root: &Path, value: &str) -> Result<(PathBuf, bool), String> {
    let input = value.trim().replace('\\', "/");
    if input.is_empty() || input.len() > 4096 || input.chars().any(|c| c.is_control() || "<>\"|?*".contains(c)) || input.starts_with("//") {
        return Err("Only local filesystem paths are supported".into());
    }
    let drive = input.as_bytes().get(0).is_some_and(u8::is_ascii_alphabetic) && input.get(1..3) == Some(":/");
    if (if drive { &input[2..] } else { &input }).contains(':') || (drive && !cfg!(windows)) || input.split('/').any(|p| p == "..") {
        return Err("Unsupported local path or parent traversal".into());
    }
    let base = root.canonicalize().map_err(|e| e.to_string())?;
    let requested = Path::new(&input);
    let selected = if requested.is_absolute() { requested.to_path_buf() } else { base.join(requested) }.canonicalize().map_err(|e| e.to_string())?;
    if !requested.is_absolute() && !selected.starts_with(&base) { return Err("Path escapes the selected folder".into()); }
    #[cfg(windows)]
    let selected = {
        let text = selected.to_string_lossy();
        let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
        if !text.as_bytes().get(0).is_some_and(u8::is_ascii_alphabetic) || text.get(1..3) != Some(":\\") { return Err("Only local drive paths are supported".into()); }
        PathBuf::from(text)
    };
    let metadata = fs::metadata(&selected).map_err(|e| e.to_string())?;
    if !metadata.is_dir() && !metadata.is_file() { return Err("Select a regular file or folder".into()); }
    Ok((selected, metadata.is_dir()))
}
fn file_manager_plan(os: &str, path: &Path, directory: bool) -> Result<(&'static str, Vec<std::ffi::OsString>), String> {
    let value = path.as_os_str().to_owned();
    match os {
        "windows" => Ok(("explorer.exe", if directory { vec![value] } else { vec!["/select,".into(), value] })),
        "macos" => Ok(("open", if directory { vec![value] } else { vec!["-R".into(), value] })),
        "linux" => Ok(("xdg-open", vec![if directory { value } else { path.parent().ok_or("File has no parent")?.as_os_str().to_owned() }])),
        _ => Err("File manager is not supported on this platform".into()),
    }
}
pub(crate) fn reveal_chat_path(root: &Path, value: &str) -> Result<(), String> {
    let (selected, directory) = resolve_chat_path(root, value)?;
    let (program, args) = file_manager_plan(std::env::consts::OS, &selected, directory)?;
    #[cfg(windows)]
    let program = PathBuf::from(std::env::var_os("SystemRoot").ok_or("SystemRoot is missing")?).join(program);
    Command::new(program).args(args).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths_are_local_and_relative_paths_cannot_escape() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("with space")).unwrap();
        fs::write(root.path().join("with space/run.cmd"), "not executed").unwrap();
        let (file, directory) = resolve_chat_path(root.path(), "with space/run.cmd").unwrap();
        assert!(!directory); assert!(file.ends_with("with space/run.cmd"));
        assert!(resolve_chat_path(root.path(), "with space").unwrap().1);
        for value in ["../outside", "C:relative", "https://example.com", r"\\server\share", "file.txt:stream", "bad\npath"] { assert!(resolve_chat_path(root.path(), value).is_err(), "{value}"); }
    }
    #[test]
    fn file_targets_are_revealed_never_executed() {
        let file = Path::new("/local/with space/run.cmd");
        let (program, args) = file_manager_plan("windows", file, false).unwrap();
        assert_eq!(program, "explorer.exe"); assert_eq!(args, vec![std::ffi::OsString::from("/select,"), file.as_os_str().to_owned()]);
        assert_eq!(file_manager_plan("windows", file, true).unwrap().1, vec![file.as_os_str().to_owned()]);
        assert_eq!(file_manager_plan("linux", file, false).unwrap().1, vec![std::ffi::OsString::from("/local/with space")]);
    }
}
