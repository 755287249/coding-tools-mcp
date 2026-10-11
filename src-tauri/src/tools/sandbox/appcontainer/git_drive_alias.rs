//! Handle-owned DOS alias for Git's AppContainer-incompatible cwd lookup.
//!
//! Temporary Object Manager links disappear when their last handle closes,
//! including host crashes. Do not use DefineDosDevice's persistent links here.
use std::ffi::c_void;
use std::io;
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::ptr;

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *mut u16,
}

impl UnicodeString {
    fn new(value: &mut [u16]) -> io::Result<Self> {
        let length = u16::try_from(value.len() * 2)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "DOS alias path too long"))?;
        Ok(Self {
            length,
            maximum_length: length,
            buffer: value.as_mut_ptr(),
        })
    }
}

#[repr(C)]
struct ObjectAttributes {
    length: u32,
    root_directory: *mut c_void,
    object_name: *mut UnicodeString,
    attributes: u32,
    security_descriptor: *mut c_void,
    security_quality_of_service: *mut c_void,
}

#[link(name = "ntdll")]
extern "system" {
    fn NtCreateSymbolicLinkObject(
        handle: *mut *mut c_void,
        access: u32,
        attributes: *mut ObjectAttributes,
        target: *mut UnicodeString,
    ) -> i32;
    fn RtlNtStatusToDosError(status: i32) -> u32;
}
#[link(name = "kernel32")]
extern "system" {
    fn GetLogicalDrives() -> u32;
}

pub(super) struct GitDriveAlias {
    _handle: OwnedHandle,
    root: PathBuf,
    drive: PathBuf,
}

impl GitDriveAlias {
    pub(super) fn create(root: &Path) -> io::Result<Self> {
        let root = root.canonicalize()?;
        let raw = root.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Non-Unicode workspace path")
        })?;
        let dos = raw.strip_prefix(r"\\?\").unwrap_or(raw);
        // UNC workspaces need a separate network identity/mapping contract.
        if dos.as_bytes().get(1) != Some(&b':') || !dos.as_bytes()[0].is_ascii_alphabetic() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Git DOS alias requires a local drive workspace",
            ));
        }
        let mut target: Vec<u16> = format!(r"\??\{dos}").encode_utf16().collect();
        let mut target = UnicodeString::new(&mut target)?;
        let used = unsafe { GetLogicalDrives() };
        if used == 0 {
            return Err(io::Error::last_os_error());
        }
        for letter in (b'D'..=b'Z').rev() {
            if used & (1 << (letter - b'A')) != 0 {
                continue;
            }
            let mut name: Vec<u16> = format!(r"\??\{}:", char::from(letter))
                .encode_utf16()
                .collect();
            let mut name = UnicodeString::new(&mut name)?;
            let mut attributes = ObjectAttributes {
                length: std::mem::size_of::<ObjectAttributes>() as u32,
                root_directory: ptr::null_mut(),
                object_name: &mut name,
                // Case-insensitive only: never open/replace an existing name,
                // inherit the handle, or create a permanent mapping.
                attributes: 0x40,
                security_descriptor: ptr::null_mut(),
                security_quality_of_service: ptr::null_mut(),
            };
            let mut handle = ptr::null_mut();
            let status =
                unsafe { NtCreateSymbolicLinkObject(&mut handle, 1, &mut attributes, &mut target) };
            if status == 0xc0000035_u32 as i32 {
                continue;
            } // atomic name collision
            if status < 0 {
                return Err(io::Error::from_raw_os_error(
                    unsafe { RtlNtStatusToDosError(status) } as i32,
                ));
            }
            let alias = Self {
                _handle: unsafe { OwnedHandle::from_raw_handle(handle) },
                root,
                drive: PathBuf::from(format!("{}:\\", char::from(letter))),
            };
            if alias.drive.canonicalize()? != alias.root {
                return Err(io::Error::other(
                    "Git DOS alias resolved to an unexpected workspace",
                ));
            }
            return Ok(alias);
        }
        Err(io::Error::other(
            "No free DOS drive letter for sandboxed Git",
        ))
    }

    pub(super) fn cwd(&self, cwd: &Path) -> io::Result<PathBuf> {
        let cwd = cwd.canonicalize()?;
        let relative = cwd.strip_prefix(&self.root).map_err(|_| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Git cwd is outside its workspace",
            )
        })?;
        Ok(self.drive.join(relative))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_are_exclusive_scoped_and_removed_on_drop() {
        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let first = GitDriveAlias::create(root.path()).unwrap();
        let second = GitDriveAlias::create(root.path()).unwrap();
        assert_ne!(first.drive, second.drive);
        assert_eq!(
            first.cwd(&nested).unwrap().canonicalize().unwrap(),
            nested.canonicalize().unwrap()
        );
        assert!(first.cwd(root.path().parent().unwrap()).is_err());
        let first_drive = first.drive.clone();
        drop(first);
        assert!(!first_drive.exists());
        assert!(second.drive.exists());
        let second_drive = second.drive.clone();
        drop(second);
        assert!(!second_drive.exists());
    }
    #[test]
    fn alias_owner_child() {
        let Some(marker) = std::env::var_os("CTMCP_GIT_ALIAS_CHILD_MARKER") else {
            return;
        };
        let root = PathBuf::from(std::env::var_os("CTMCP_GIT_ALIAS_CHILD_ROOT").unwrap());
        let alias = GitDriveAlias::create(&root).unwrap();
        std::fs::write(marker, alias.drive.to_str().unwrap()).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(30));
    }

    #[test]
    fn terminating_alias_owner_does_not_leave_a_drive_mapping() {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("drive.txt");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tools::sandbox::appcontainer::git_drive_alias::tests::alias_owner_child",
            ])
            .env("CTMCP_GIT_ALIAS_CHILD_ROOT", root.path())
            .env("CTMCP_GIT_ALIAS_CHILD_MARKER", &marker)
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let drive = loop {
            if let Ok(value) = std::fs::read_to_string(&marker) {
                if !value.is_empty() {
                    break Some(PathBuf::from(value));
                }
            }
            if std::time::Instant::now() >= deadline {
                break None;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        let existed = drive.as_ref().is_some_and(|p| p.exists());
        let _ = child.kill();
        child.wait().unwrap();
        assert!(existed, "child did not publish a usable alias");
        assert!(
            !drive.unwrap().exists(),
            "kernel must release alias on owner termination"
        );
    }
}
