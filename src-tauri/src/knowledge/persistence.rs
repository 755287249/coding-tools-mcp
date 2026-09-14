use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

pub(super) fn write_private_atomic(root: &Path, path: &Path, contents: &[u8]) -> io::Result<()> {
    fs::create_dir_all(root)?;
    let temp_path = path.with_extension("json.tmp");
    write_private_file(&temp_path, contents)?;
    fs::rename(temp_path, path)?;
    Ok(())
}

#[cfg(unix)]
fn write_private_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents)
}

#[cfg(not(unix))]
fn write_private_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    file.write_all(contents)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn atomic_write_replaces_contents() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("knowledge.json");
        write_private_atomic(dir.path(), &path, b"first\n").unwrap();
        write_private_atomic(dir.path(), &path, b"second\n").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second\n");
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_uses_private_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let path = dir.path().join("knowledge.json");
        write_private_atomic(dir.path(), &path, b"secret\n").unwrap();
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
