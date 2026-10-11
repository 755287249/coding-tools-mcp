use std::collections::BTreeMap;
use std::ffi::c_void;
use std::mem;
use std::os::windows::io::{FromRawHandle, OwnedHandle, RawHandle};
use std::ptr;
use std::sync::Arc;

use windows::core::{HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, LocalFree, SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, HLOCAL,
};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::Isolation::{CreateAppContainerProfile, DeleteAppContainerProfile};
use windows::Win32::Security::{
    FreeSid, GetTokenInformation, TokenAppContainerSid, TokenCapabilities, TokenIsAppContainer,
    PSID, SECURITY_ATTRIBUTES, SECURITY_CAPABILITIES, SID_AND_ATTRIBUTES,
    TOKEN_APPCONTAINER_INFORMATION, TOKEN_GROUPS, TOKEN_QUERY,
};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::SystemServices::SE_GROUP_ENABLED;
use windows::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, InitializeProcThreadAttributeList,
    OpenProcessToken, ResumeThread, TerminateProcess, UpdateProcThreadAttribute, CREATE_NO_WINDOW,
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
    STARTF_USESTDHANDLES, STARTUPINFOEXW,
};

use crate::platform::attach_process_tree_handle;
use crate::tools::process_child::ProcessChild;
use crate::tools::process_spec::ProcessLaunchSpec;
use crate::tools::workspace::{WorkspaceError, WorkspaceResult};

mod git_drive_alias;
mod provider;

pub(super) use provider::prepare;
pub(crate) use provider::run_acl_helper_if_requested;

pub(super) struct AppContainerProfile {
    moniker: HSTRING,
    sid: PSID,
}

// The SID memory is immutable after creation and is only released by Drop. The
// profile value moves with ProcessChild and is never concurrently mutated.
unsafe impl Send for AppContainerProfile {}
unsafe impl Sync for AppContainerProfile {}

impl AppContainerProfile {
    pub(super) fn create(moniker: &str, capability_sid: Option<PSID>) -> WorkspaceResult<Self> {
        let moniker = HSTRING::from(moniker);
        let display = HSTRING::from("Coding Tools MCP Sandbox");
        let description = HSTRING::from("Coding Tools MCP isolated workspace process");
        let capability = capability_sid.map(|sid| SID_AND_ATTRIBUTES {
            Sid: sid,
            Attributes: SE_GROUP_ENABLED as u32,
        });
        let capabilities = capability.as_ref().map(std::slice::from_ref);
        let sid =
            unsafe { CreateAppContainerProfile(&moniker, &display, &description, capabilities) }
                .map_err(|error| {
                    appcontainer_error("SANDBOX_PROFILE_CREATE_FAILED", error.to_string())
                })?;
        Ok(Self { moniker, sid })
    }

    pub(super) fn sid(&self) -> PSID {
        self.sid
    }
}

impl Drop for AppContainerProfile {
    fn drop(&mut self) {
        unsafe {
            let _ = FreeSid(self.sid);
            let _ = DeleteAppContainerProfile(&self.moniker);
        }
    }
}

struct AttributeList {
    list: LPPROC_THREAD_ATTRIBUTE_LIST,
    _bytes: Vec<u8>,
    _capabilities: Vec<SID_AND_ATTRIBUTES>,
    _security_capabilities: Box<SECURITY_CAPABILITIES>,
}

impl AttributeList {
    fn security_capabilities(
        appcontainer_sid: PSID,
        capability_sids: &[PSID],
    ) -> WorkspaceResult<Self> {
        let mut size = 0usize;
        let _ = unsafe { InitializeProcThreadAttributeList(None, 1, None, &mut size) };
        if size == 0 {
            return Err(appcontainer_error(
                "SANDBOX_ATTRIBUTE_LIST_FAILED",
                "Windows returned a zero process attribute-list size.",
            ));
        }
        let mut bytes = vec![0u8; size];
        let list = LPPROC_THREAD_ATTRIBUTE_LIST(bytes.as_mut_ptr().cast());
        unsafe { InitializeProcThreadAttributeList(Some(list), 1, None, &mut size) }.map_err(
            |error| appcontainer_error("SANDBOX_ATTRIBUTE_LIST_FAILED", error.to_string()),
        )?;

        let mut capabilities = capability_sids
            .iter()
            .copied()
            .map(|sid| SID_AND_ATTRIBUTES {
                Sid: sid,
                Attributes: SE_GROUP_ENABLED as u32,
            })
            .collect::<Vec<_>>();
        let mut security_capabilities = Box::new(SECURITY_CAPABILITIES {
            AppContainerSid: appcontainer_sid,
            Capabilities: if capabilities.is_empty() {
                ptr::null_mut()
            } else {
                capabilities.as_mut_ptr()
            },
            CapabilityCount: capabilities.len() as u32,
            Reserved: 0,
        });
        let update = unsafe {
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
                Some(
                    (security_capabilities.as_mut() as *mut SECURITY_CAPABILITIES).cast::<c_void>(),
                ),
                mem::size_of::<SECURITY_CAPABILITIES>(),
                None,
                None,
            )
        };
        if let Err(error) = update {
            unsafe { DeleteProcThreadAttributeList(list) };
            return Err(appcontainer_error(
                "SANDBOX_SECURITY_CAPABILITIES_FAILED",
                error.to_string(),
            ));
        }
        Ok(Self {
            list,
            _bytes: bytes,
            _capabilities: capabilities,
            _security_capabilities: security_capabilities,
        })
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        unsafe { DeleteProcThreadAttributeList(self.list) };
    }
}

struct StdioPipes {
    child_stdin: HANDLE,
    parent_stdin: HANDLE,
    parent_stdout: HANDLE,
    child_stdout: HANDLE,
    parent_stderr: HANDLE,
    child_stderr: HANDLE,
}

impl StdioPipes {
    fn new() -> WorkspaceResult<Self> {
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: ptr::null_mut(),
            bInheritHandle: true.into(),
        };
        let mut pipes = Self {
            child_stdin: HANDLE::default(),
            parent_stdin: HANDLE::default(),
            parent_stdout: HANDLE::default(),
            child_stdout: HANDLE::default(),
            parent_stderr: HANDLE::default(),
            child_stderr: HANDLE::default(),
        };
        unsafe {
            CreatePipe(
                &mut pipes.child_stdin,
                &mut pipes.parent_stdin,
                Some(&mut attributes),
                0,
            )
            .map_err(|error| appcontainer_error("SANDBOX_STDIO_FAILED", error.to_string()))?;
            CreatePipe(
                &mut pipes.parent_stdout,
                &mut pipes.child_stdout,
                Some(&mut attributes),
                0,
            )
            .map_err(|error| appcontainer_error("SANDBOX_STDIO_FAILED", error.to_string()))?;
            CreatePipe(
                &mut pipes.parent_stderr,
                &mut pipes.child_stderr,
                Some(&mut attributes),
                0,
            )
            .map_err(|error| appcontainer_error("SANDBOX_STDIO_FAILED", error.to_string()))?;
            SetHandleInformation(
                pipes.parent_stdin,
                HANDLE_FLAG_INHERIT.0,
                Default::default(),
            )
            .map_err(|error| appcontainer_error("SANDBOX_STDIO_FAILED", error.to_string()))?;
            SetHandleInformation(
                pipes.parent_stdout,
                HANDLE_FLAG_INHERIT.0,
                Default::default(),
            )
            .map_err(|error| appcontainer_error("SANDBOX_STDIO_FAILED", error.to_string()))?;
            SetHandleInformation(
                pipes.parent_stderr,
                HANDLE_FLAG_INHERIT.0,
                Default::default(),
            )
            .map_err(|error| appcontainer_error("SANDBOX_STDIO_FAILED", error.to_string()))?;
        }
        Ok(pipes)
    }

    fn close_child_ends(&mut self) {
        close_handle(&mut self.child_stdin);
        close_handle(&mut self.child_stdout);
        close_handle(&mut self.child_stderr);
    }

    unsafe fn take_parent_stdin(&mut self) -> OwnedHandle {
        take_owned_handle(&mut self.parent_stdin)
    }

    unsafe fn take_parent_stdout(&mut self) -> OwnedHandle {
        take_owned_handle(&mut self.parent_stdout)
    }

    unsafe fn take_parent_stderr(&mut self) -> OwnedHandle {
        take_owned_handle(&mut self.parent_stderr)
    }
}

impl Drop for StdioPipes {
    fn drop(&mut self) {
        close_handle(&mut self.child_stdin);
        close_handle(&mut self.parent_stdin);
        close_handle(&mut self.parent_stdout);
        close_handle(&mut self.child_stdout);
        close_handle(&mut self.parent_stderr);
        close_handle(&mut self.child_stderr);
    }
}

fn verify_process_appcontainer_token(
    process: HANDLE,
    expected_sid: PSID,
    expected_capability_sids: &[PSID],
) -> WorkspaceResult<()> {
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }
        .map_err(|error| appcontainer_error("SANDBOX_TOKEN_QUERY_FAILED", error.to_string()))?;

    let result = (|| -> WorkspaceResult<()> {
        let mut is_appcontainer = 0u32;
        let mut returned = 0u32;
        unsafe {
            GetTokenInformation(
                token,
                TokenIsAppContainer,
                Some((&mut is_appcontainer as *mut u32).cast::<c_void>()),
                mem::size_of::<u32>() as u32,
                &mut returned,
            )
        }
        .map_err(|error| appcontainer_error("SANDBOX_TOKEN_QUERY_FAILED", error.to_string()))?;
        if is_appcontainer == 0 {
            return Err(appcontainer_error(
                "SANDBOX_TOKEN_VERIFICATION_FAILED",
                "Created process is not running with an AppContainer token.",
            ));
        }

        let mut required = 0u32;
        let _ = unsafe { GetTokenInformation(token, TokenAppContainerSid, None, 0, &mut required) };
        if required < mem::size_of::<TOKEN_APPCONTAINER_INFORMATION>() as u32 {
            return Err(appcontainer_error(
                "SANDBOX_TOKEN_QUERY_FAILED",
                "Windows returned an invalid AppContainer token-information size.",
            ));
        }
        let mut buffer = vec![0u8; required as usize];
        unsafe {
            GetTokenInformation(
                token,
                TokenAppContainerSid,
                Some(buffer.as_mut_ptr().cast::<c_void>()),
                required,
                &mut required,
            )
        }
        .map_err(|error| appcontainer_error("SANDBOX_TOKEN_QUERY_FAILED", error.to_string()))?;
        let info = unsafe {
            ptr::read_unaligned(buffer.as_ptr().cast::<TOKEN_APPCONTAINER_INFORMATION>())
        };
        if info.TokenAppContainer.is_invalid() {
            return Err(appcontainer_error(
                "SANDBOX_TOKEN_VERIFICATION_FAILED",
                "Created process AppContainer token did not contain a package SID.",
            ));
        }
        let actual = sid_string(info.TokenAppContainer)?;
        let expected = sid_string(expected_sid)?;
        if !actual.eq_ignore_ascii_case(&expected) {
            return Err(appcontainer_error(
                "SANDBOX_TOKEN_VERIFICATION_FAILED",
                format!("Created process AppContainer SID {actual} does not match sandbox profile SID {expected}."),
            ));
        }

        if !expected_capability_sids.is_empty() {
            let mut capabilities_required = 0u32;
            let _ = unsafe {
                GetTokenInformation(
                    token,
                    TokenCapabilities,
                    None,
                    0,
                    &mut capabilities_required,
                )
            };
            if capabilities_required < mem::size_of::<TOKEN_GROUPS>() as u32 {
                return Err(appcontainer_error(
                    "SANDBOX_TOKEN_QUERY_FAILED",
                    "Windows returned an invalid token-capabilities information size.",
                ));
            }
            let mut capabilities_buffer = vec![0u8; capabilities_required as usize];
            unsafe {
                GetTokenInformation(
                    token,
                    TokenCapabilities,
                    Some(capabilities_buffer.as_mut_ptr().cast::<c_void>()),
                    capabilities_required,
                    &mut capabilities_required,
                )
            }
            .map_err(|error| appcontainer_error("SANDBOX_TOKEN_QUERY_FAILED", error.to_string()))?;
            let groups = capabilities_buffer.as_ptr().cast::<TOKEN_GROUPS>();
            let group_count = unsafe { (*groups).GroupCount as usize };
            let group_base = unsafe { (*groups).Groups.as_ptr() };
            let actual_capabilities = (0..group_count)
                .map(|index| unsafe { *group_base.add(index) })
                .map(|entry| sid_string(entry.Sid))
                .collect::<WorkspaceResult<Vec<_>>>()?;
            for expected_sid in expected_capability_sids {
                let expected = sid_string(*expected_sid)?;
                if !actual_capabilities
                    .iter()
                    .any(|actual| actual.eq_ignore_ascii_case(&expected))
                {
                    return Err(appcontainer_error(
                        "SANDBOX_TOKEN_VERIFICATION_FAILED",
                        format!(
                            "Created process AppContainer token is missing expected capability SID {expected}; actual capabilities: {}.",
                            actual_capabilities.join(", ")
                        ),
                    ));
                }
            }
        }
        Ok(())
    })();

    unsafe {
        let _ = CloseHandle(token);
    }
    result
}

pub(super) fn launch_process(
    process_spec: &ProcessLaunchSpec,
    sandbox_environment_overrides: &BTreeMap<String, String>,
    profile: Arc<AppContainerProfile>,
    capability_sids: &[PSID],
) -> WorkspaceResult<ProcessChild> {
    if process_spec.using_wsl {
        return Err(appcontainer_error(
            "SANDBOX_BACKEND_UNSUPPORTED",
            "Windows AppContainer cannot launch a WSL-normalized process. Use Docker Sandboxes or WSL Containers for WSL folders.",
        ));
    }
    let cwd = process_spec.cwd.as_ref().ok_or_else(|| {
        appcontainer_error(
            "SANDBOX_PROCESS_SPEC_INVALID",
            "AppContainer host process requires an explicit working directory.",
        )
    })?;
    let attributes = AttributeList::security_capabilities(profile.sid(), capability_sids)?;
    let mut pipes = StdioPipes::new()?;
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = pipes.child_stdin;
    startup.StartupInfo.hStdOutput = pipes.child_stdout;
    startup.StartupInfo.hStdError = pipes.child_stderr;
    startup.lpAttributeList = attributes.list;

    let command_line = build_command_line(process_spec);
    let mut command_line_w = command_line
        .encode_utf16()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let application = HSTRING::from(process_spec.program.to_string_lossy().as_ref());
    let current_dir = HSTRING::from(cwd.to_string_lossy().as_ref());
    let environment_block = build_environment_block(process_spec, sandbox_environment_overrides);
    let mut process = PROCESS_INFORMATION::default();
    unsafe {
        CreateProcessW(
            &application,
            Some(PWSTR(command_line_w.as_mut_ptr())),
            None,
            None,
            true,
            EXTENDED_STARTUPINFO_PRESENT
                | CREATE_NO_WINDOW
                | CREATE_SUSPENDED
                | CREATE_UNICODE_ENVIRONMENT,
            Some(environment_block.as_ptr().cast::<c_void>()),
            &current_dir,
            &startup.StartupInfo,
            &mut process,
        )
    }
    .map_err(|error| appcontainer_error("SANDBOX_PROCESS_CREATE_FAILED", error.to_string()))?;

    if let Err(error) =
        verify_process_appcontainer_token(process.hProcess, profile.sid(), capability_sids)
    {
        unsafe {
            let _ = TerminateProcess(process.hProcess, 1);
            let _ = CloseHandle(process.hThread);
            let _ = CloseHandle(process.hProcess);
        }
        return Err(error);
    }

    pipes.close_child_ends();
    let Some(tree) = attach_process_tree_handle(process.hProcess) else {
        unsafe {
            let _ = TerminateProcess(process.hProcess, 1);
            let _ = CloseHandle(process.hThread);
            let _ = CloseHandle(process.hProcess);
        }
        return Err(appcontainer_error(
            "SANDBOX_PROCESS_CONTAINMENT_FAILED",
            "Failed to assign the suspended AppContainer process to its kill-on-close Job Object.",
        ));
    };
    let resumed = unsafe { ResumeThread(process.hThread) };
    unsafe {
        let _ = CloseHandle(process.hThread);
    }
    if resumed == u32::MAX {
        unsafe {
            let _ = TerminateProcess(process.hProcess, 1);
            let _ = CloseHandle(process.hProcess);
        }
        return Err(appcontainer_error(
            "SANDBOX_PROCESS_RESUME_FAILED",
            "ResumeThread failed for the contained AppContainer process.",
        ));
    }

    let process_handle = unsafe { OwnedHandle::from_raw_handle(process.hProcess.0 as RawHandle) };
    let child = ProcessChild::from_windows_handles(
        process_handle,
        process.dwProcessId,
        Some(unsafe { pipes.take_parent_stdin() }),
        Some(unsafe { pipes.take_parent_stdout() }),
        Some(unsafe { pipes.take_parent_stderr() }),
        tree,
    )
    .with_backend_lifetime(profile);
    Ok(child)
}

fn build_command_line(process_spec: &ProcessLaunchSpec) -> String {
    let mut parts = std::iter::once(quote_windows_text(&process_spec.program.to_string_lossy()))
        .chain(
            process_spec
                .args
                .iter()
                .map(|argument| quote_windows_text(argument)),
        )
        .collect::<Vec<_>>();
    if let Some(raw_argument) = process_spec.windows_raw_arg.as_deref() {
        parts.push(raw_argument.to_string());
    }
    parts.join(" ")
}

fn build_environment_block(
    process_spec: &ProcessLaunchSpec,
    sandbox_overrides: &BTreeMap<String, String>,
) -> Vec<u16> {
    let mut values = BTreeMap::<String, (String, String)>::new();
    for (key, value) in std::env::vars() {
        values.insert(key.to_ascii_uppercase(), (key, value));
    }
    for (key, value) in &process_spec.env {
        values.insert(key.to_ascii_uppercase(), (key.clone(), value.clone()));
    }
    for key in &process_spec.remove_env {
        values.remove(&key.to_ascii_uppercase());
    }
    for (key, value) in &process_spec.required_env {
        values.insert(key.to_ascii_uppercase(), (key.clone(), value.clone()));
    }
    // Sandbox-owned state is the final layer. A caller may not redirect HOME/TEMP/cache
    // back to the real user profile by supplying or removing the same environment keys.
    for (key, value) in sandbox_overrides {
        values.insert(key.to_ascii_uppercase(), (key.clone(), value.clone()));
    }
    let mut block = Vec::new();
    for (_, (key, value)) in values {
        block.extend(format!("{key}={value}").encode_utf16());
        block.push(0);
    }
    block.push(0);
    block
}

fn quote_windows_text(value: &str) -> String {
    if value.is_empty() {
        return "\"\"".into();
    }
    if !value.chars().any(|ch| ch.is_whitespace() || ch == '"') {
        return value.to_string();
    }

    let mut output = String::from("\"");
    let mut backslashes = 0usize;
    for ch in value.chars() {
        match ch {
            '\\' => backslashes += 1,
            '"' => {
                output.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                output.push('"');
                backslashes = 0;
            }
            _ => {
                output.extend(std::iter::repeat_n('\\', backslashes));
                backslashes = 0;
                output.push(ch);
            }
        }
    }
    output.extend(std::iter::repeat_n('\\', backslashes * 2));
    output.push('"');
    output
}

fn close_handle(handle: &mut HANDLE) {
    if !handle.is_invalid() {
        unsafe {
            let _ = CloseHandle(*handle);
        }
        *handle = HANDLE::default();
    }
}

unsafe fn take_owned_handle(handle: &mut HANDLE) -> OwnedHandle {
    let owned = OwnedHandle::from_raw_handle(handle.0 as RawHandle);
    *handle = HANDLE::default();
    owned
}

pub(super) fn sid_string(sid: PSID) -> WorkspaceResult<String> {
    let mut string_sid = PWSTR::null();
    unsafe { ConvertSidToStringSidW(sid, &mut string_sid) }
        .map_err(|error| appcontainer_error("SANDBOX_SID_CONVERSION_FAILED", error.to_string()))?;
    let text = unsafe { PCWSTR(string_sid.0).to_string() }
        .map_err(|error| appcontainer_error("SANDBOX_SID_CONVERSION_FAILED", error.to_string()))?;
    unsafe {
        let _ = LocalFree(Some(HLOCAL(string_sid.0.cast())));
    }
    Ok(text)
}

fn appcontainer_error(code: &'static str, message: impl Into<String>) -> WorkspaceError {
    WorkspaceError::ToolDetails {
        code,
        message: message.into(),
        category: "security",
        retryable: false,
        details: serde_json::json!({
            "sandbox_enabled": true,
            "sandbox_backend": "appcontainer",
            "sandbox_status": "launch_failed",
            "fallback_allowed": false,
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    use windows::Win32::System::Threading::GetCurrentProcess;

    use super::*;

    #[test]
    fn appcontainer_child_probe() {
        if std::env::var_os("CTMCP_APPCONTAINER_CHILD_TEST").is_none() {
            return;
        }
        assert!(current_process_is_appcontainer().expect("token query"));
        if let Ok(expected_sid) = std::env::var("CTMCP_APPCONTAINER_EXPECTED_SID") {
            let actual_sid =
                current_process_appcontainer_sid_string().expect("AppContainer SID query");
            assert_eq!(
                actual_sid, expected_sid,
                "unexpected AppContainer package SID"
            );
        }

        if std::env::var_os("CTMCP_APPCONTAINER_GRANDCHILD").is_some() {
            fs::write("grandchild-inside-created.txt", "contained")
                .expect("grandchild write inside workspace");
            assert!(fs::write(
                Path::new("..").join("grandchild-outside-blocked.txt"),
                "escape"
            )
            .is_err());
            println!("appcontainer-grandchild-ok");
            return;
        }

        if let Ok(delay_ms) = std::env::var("CTMCP_APPCONTAINER_CHILD_DELAY_MS") {
            let delay_ms = delay_ms.parse::<u64>().expect("delay ms");
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        }
        fs::write("inside-created.txt", "contained").expect("write inside workspace");
        fs::write("inside-delete.txt", "delete-me").expect("create deletable workspace file");
        fs::remove_file("inside-delete.txt").expect("delete inside workspace");
        assert!(!Path::new("inside-delete.txt").exists());
        assert!(fs::write(Path::new("..").join("outside-blocked.txt"), "escape").is_err());
        assert!(fs::write(Path::new("..").join("outside-replace.txt"), "replaced").is_err());
        assert!(fs::remove_file(Path::new("..").join("outside-delete.txt")).is_err());

        let grandchild_status = Command::new(std::env::current_exe().expect("current executable"))
            .args([
                "--exact",
                "tools::sandbox::appcontainer::tests::appcontainer_child_probe",
                "--nocapture",
            ])
            .env("CTMCP_APPCONTAINER_GRANDCHILD", "1")
            .status()
            .expect("spawn AppContainer grandchild");
        assert!(
            grandchild_status.success(),
            "grandchild failed: {grandchild_status}"
        );
        println!("appcontainer-child-ok");
    }

    #[test]
    fn process_command_line_preserves_windows_raw_argument_tail() {
        let raw_tail = r#"call "C:\workspace with spaces\run.cmd" "a&b""#;
        let process_spec = ProcessLaunchSpec {
            program: PathBuf::from(r"C:\Windows\System32\cmd.exe"),
            args: vec!["/d".into(), "/s".into(), "/c".into()],
            cwd: Some(PathBuf::from(r"C:\workspace with spaces")),
            env: Vec::new(),
            remove_env: Vec::new(),
            required_env: Vec::new(),
            windows_raw_arg: Some(raw_tail.into()),
            using_wsl: false,
        };
        let command_line = build_command_line(&process_spec);
        assert!(command_line.starts_with(r"C:\Windows\System32\cmd.exe /d /s /c "));
        assert!(command_line.ends_with(raw_tail));
        assert_eq!(command_line.matches(raw_tail).count(), 1);
    }

    #[test]
    fn sandbox_environment_overrides_win_after_caller_remove_and_required_layers() {
        let process_spec = ProcessLaunchSpec {
            program: PathBuf::from("tool.exe"),
            args: Vec::new(),
            cwd: Some(PathBuf::from(r"C:\workspace")),
            env: vec![
                ("TEMP".into(), "caller-temp".into()),
                ("HOME".into(), "caller-home".into()),
                ("CTMCP_ENV_ORDER".into(), "caller".into()),
                ("CTMCP_REMOVE_ONLY".into(), "remove-me".into()),
            ],
            remove_env: vec![
                "home".into(),
                "ctmcp_env_order".into(),
                "CTMCP_REMOVE_ONLY".into(),
            ],
            required_env: vec![
                ("HOME".into(), "required-home".into()),
                ("CTMCP_ENV_ORDER".into(), "required".into()),
            ],
            windows_raw_arg: None,
            using_wsl: false,
        };
        let sandbox = BTreeMap::from([
            ("temp".into(), "sandbox-temp".into()),
            ("Home".into(), "sandbox-home".into()),
        ]);
        let values = decode_environment_block(&build_environment_block(&process_spec, &sandbox));

        assert_eq!(values.get("TEMP").map(String::as_str), Some("sandbox-temp"));
        assert_eq!(values.get("HOME").map(String::as_str), Some("sandbox-home"));
        assert_eq!(
            values.get("CTMCP_ENV_ORDER").map(String::as_str),
            Some("required")
        );
        assert!(!values.contains_key("CTMCP_REMOVE_ONLY"));
    }

    #[tokio::test]
    async fn system_cmd_runs_without_private_runtime_acl() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_millis();
        let root = tempfile::tempdir().expect("temp root");
        let inside = root.path().join("inside");
        fs::create_dir_all(&inside).expect("inside");
        let script = inside.join("system-cmd-probe.cmd");
        fs::write(&script, "@echo appcontainer-cmd-ok\r\n").expect("cmd probe");

        let moniker = format!(
            "CodingToolsMcp.Sandbox.SystemCmd.{}.{}",
            std::process::id(),
            nonce
        );
        let profile = Arc::new(AppContainerProfile::create(&moniker, None).expect("profile"));
        let sid = sid_string(profile.sid()).expect("sid string");
        grant_acl(root.path(), &sid, "(OI)(CI)RX").expect("root read/traverse");
        grant_acl(&inside, &sid, "(OI)(CI)M").expect("workspace modify");

        let process_spec = ProcessLaunchSpec {
            program: PathBuf::from(r"C:\Windows\System32\cmd.exe"),
            args: vec!["/d".into(), "/s".into(), "/c".into()],
            cwd: Some(inside),
            env: Vec::new(),
            remove_env: Vec::new(),
            required_env: Vec::new(),
            windows_raw_arg: Some(format!(
                "call {}",
                quote_windows_text(&script.to_string_lossy())
            )),
            using_wsl: false,
        };
        let child = launch_process(&process_spec, &BTreeMap::new(), profile, &[])
            .expect("system cmd launch");
        let output = child.wait_with_output().await.expect("cmd output");
        assert_eq!(output.status.code(), Some(0));
        assert!(String::from_utf8_lossy(&output.stdout).contains("appcontainer-cmd-ok"));
    }

    #[tokio::test]
    async fn powershell_core_runs_without_private_runtime_acl() {
        let pwsh = which::which("pwsh.exe").expect("pwsh.exe");
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_millis();
        let root = tempfile::tempdir().expect("temp root");
        let inside = root.path().join("inside");
        fs::create_dir_all(&inside).expect("inside");

        let moniker = format!(
            "CodingToolsMcp.Sandbox.PowerShellCore.{}.{}",
            std::process::id(),
            nonce
        );
        let profile = Arc::new(AppContainerProfile::create(&moniker, None).expect("profile"));
        let sid = sid_string(profile.sid()).expect("sid string");
        grant_acl(root.path(), &sid, "(OI)(CI)RX").expect("root read/traverse");
        grant_acl(&inside, &sid, "(OI)(CI)M").expect("workspace modify");

        let process_spec = ProcessLaunchSpec {
            program: pwsh,
            args: vec![
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                "Write-Output 'appcontainer-pwsh-ok'".into(),
            ],
            cwd: Some(inside),
            env: Vec::new(),
            remove_env: Vec::new(),
            required_env: Vec::new(),
            windows_raw_arg: None,
            using_wsl: false,
        };
        let child = launch_process(&process_spec, &BTreeMap::new(), profile, &[])
            .expect("PowerShell Core launch");
        let output = child.wait_with_output().await.expect("pwsh output");
        assert_eq!(
            output.status.code(),
            Some(0),
            "stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("appcontainer-pwsh-ok"));
    }

    #[tokio::test]
    async fn production_launcher_creates_contained_appcontainer_child() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_millis();
        let root = tempfile::tempdir().expect("temp root");
        let inside = root.path().join("inside");
        fs::create_dir_all(&inside).expect("inside");
        fs::write(root.path().join("outside-replace.txt"), "original")
            .expect("outside replace fixture");
        fs::write(root.path().join("outside-delete.txt"), "preserve")
            .expect("outside delete fixture");
        let child_exe = inside.join("appcontainer-launcher-test.exe");
        fs::copy(std::env::current_exe().expect("current exe"), &child_exe).expect("copy child");

        let moniker = format!(
            "CodingToolsMcp.Sandbox.ProductionLauncher.{}.{}",
            std::process::id(),
            nonce
        );
        let profile = Arc::new(AppContainerProfile::create(&moniker, None).expect("profile"));
        let sid = sid_string(profile.sid()).expect("sid string");
        grant_acl(root.path(), &sid, "(OI)(CI)RX").expect("root read/traverse");
        grant_acl(&inside, &sid, "(OI)(CI)M").expect("workspace modify");

        let process_spec = ProcessLaunchSpec {
            program: child_exe,
            args: vec![
                "--exact".into(),
                "tools::sandbox::appcontainer::tests::appcontainer_child_probe".into(),
                "--nocapture".into(),
            ],
            cwd: Some(inside.clone()),
            env: vec![
                ("CTMCP_APPCONTAINER_CHILD_TEST".into(), "1".into()),
                ("CTMCP_APPCONTAINER_EXPECTED_SID".into(), sid.clone()),
            ],
            remove_env: Vec::new(),
            required_env: Vec::new(),
            windows_raw_arg: None,
            using_wsl: false,
        };
        let child =
            launch_process(&process_spec, &BTreeMap::new(), profile, &[]).expect("sandbox launch");
        assert!(child.process_tree_contained());
        let output = child.wait_with_output().await.expect("sandbox output");
        assert_eq!(output.status.code(), Some(0));
        assert!(String::from_utf8_lossy(&output.stdout).contains("appcontainer-child-ok"));
        assert!(String::from_utf8_lossy(&output.stdout).contains("appcontainer-grandchild-ok"));
        assert!(inside.join("inside-created.txt").exists());
        assert!(!inside.join("inside-delete.txt").exists());
        assert!(inside.join("grandchild-inside-created.txt").exists());
        assert!(!root.path().join("outside-blocked.txt").exists());
        assert!(!root.path().join("grandchild-outside-blocked.txt").exists());
        assert_eq!(
            fs::read_to_string(root.path().join("outside-replace.txt")).expect("outside replace"),
            "original"
        );
        assert_eq!(
            fs::read_to_string(root.path().join("outside-delete.txt")).expect("outside delete"),
            "preserve"
        );
    }

    #[test]
    fn profile_creation_failure_is_structured_and_disallows_fallback() {
        let error = match AppContainerProfile::create("", None) {
            Ok(profile) => {
                drop(profile);
                panic!("empty AppContainer moniker unexpectedly succeeded")
            }
            Err(error) => error,
        };
        match error {
            WorkspaceError::ToolDetails {
                code,
                category,
                retryable,
                details,
                ..
            } => {
                assert_eq!(code, "SANDBOX_PROFILE_CREATE_FAILED");
                assert_eq!(category, "security");
                assert!(!retryable);
                assert_eq!(details["sandbox_backend"], "appcontainer");
                assert_eq!(details["sandbox_status"], "launch_failed");
                assert_eq!(details["fallback_allowed"], false);
            }
            other => panic!("unexpected profile error: {other:?}"),
        }
    }

    #[test]
    fn duplicate_profile_creation_fails_closed_instead_of_deriving_an_existing_sid() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let moniker = format!("CodingToolsMcp.Dup.{}.{}", std::process::id(), nonce);
        let profile = AppContainerProfile::create(&moniker, None).expect("first profile");
        let error = match AppContainerProfile::create(&moniker, None) {
            Ok(duplicate) => {
                drop(duplicate);
                drop(profile);
                panic!("duplicate AppContainer profile unexpectedly succeeded")
            }
            Err(error) => error,
        };
        drop(profile);
        match error {
            WorkspaceError::ToolDetails {
                code,
                retryable,
                details,
                ..
            } => {
                assert_eq!(code, "SANDBOX_PROFILE_CREATE_FAILED");
                assert!(!retryable);
                assert_eq!(details["fallback_allowed"], false);
            }
            other => panic!("unexpected duplicate profile error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn process_creation_failure_is_structured_and_disallows_fallback() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_millis();
        let root = tempfile::tempdir().expect("temp root");
        let inside = root.path().join("inside");
        fs::create_dir_all(&inside).expect("inside");
        let moniker = format!(
            "CodingToolsMcp.Sandbox.ProcessFailure.{}.{}",
            std::process::id(),
            nonce
        );
        let profile = Arc::new(AppContainerProfile::create(&moniker, None).expect("profile"));
        let sid = sid_string(profile.sid()).expect("sid string");
        grant_acl(root.path(), &sid, "(OI)(CI)RX").expect("root read/traverse");
        grant_acl(&inside, &sid, "(OI)(CI)M").expect("workspace modify");

        let process_spec = ProcessLaunchSpec {
            program: inside.join("does-not-exist.exe"),
            args: Vec::new(),
            cwd: Some(inside),
            env: Vec::new(),
            remove_env: Vec::new(),
            required_env: Vec::new(),
            windows_raw_arg: None,
            using_wsl: false,
        };
        let error = match launch_process(&process_spec, &BTreeMap::new(), profile, &[]) {
            Ok(child) => {
                drop(child);
                panic!("missing AppContainer executable unexpectedly launched")
            }
            Err(error) => error,
        };
        match error {
            WorkspaceError::ToolDetails {
                code,
                category,
                retryable,
                details,
                ..
            } => {
                assert_eq!(code, "SANDBOX_PROCESS_CREATE_FAILED");
                assert_eq!(category, "security");
                assert!(!retryable);
                assert_eq!(details["sandbox_backend"], "appcontainer");
                assert_eq!(details["sandbox_status"], "launch_failed");
                assert_eq!(details["fallback_allowed"], false);
            }
            other => panic!("unexpected process-create error: {other:?}"),
        }
    }

    fn decode_environment_block(block: &[u16]) -> BTreeMap<String, String> {
        let mut values = BTreeMap::new();
        for entry in block.split(|value| *value == 0) {
            if entry.is_empty() {
                break;
            }
            let entry = String::from_utf16(entry).expect("environment entry");
            if let Some((key, value)) = entry.split_once('=') {
                if !key.is_empty() {
                    values.insert(key.to_ascii_uppercase(), value.to_string());
                }
            }
        }
        values
    }

    fn current_process_is_appcontainer() -> Result<bool, windows::core::Error> {
        let mut token = HANDLE::default();
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)? };
        let mut value = 0u32;
        let mut returned = 0u32;
        let result = unsafe {
            GetTokenInformation(
                token,
                TokenIsAppContainer,
                Some((&mut value as *mut u32).cast::<c_void>()),
                mem::size_of::<u32>() as u32,
                &mut returned,
            )
        };
        unsafe {
            let _ = CloseHandle(token);
        }
        result?;
        Ok(value != 0)
    }

    fn current_process_appcontainer_sid_string() -> WorkspaceResult<String> {
        let mut token = HANDLE::default();
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }
            .map_err(|error| appcontainer_error("SANDBOX_TOKEN_QUERY_FAILED", error.to_string()))?;

        let mut required = 0u32;
        let _ = unsafe { GetTokenInformation(token, TokenAppContainerSid, None, 0, &mut required) };
        if required < mem::size_of::<TOKEN_APPCONTAINER_INFORMATION>() as u32 {
            unsafe {
                let _ = CloseHandle(token);
            }
            return Err(appcontainer_error(
                "SANDBOX_TOKEN_QUERY_FAILED",
                "Windows returned an invalid AppContainer token-information size.",
            ));
        }

        let mut buffer = vec![0u8; required as usize];
        let query = unsafe {
            GetTokenInformation(
                token,
                TokenAppContainerSid,
                Some(buffer.as_mut_ptr().cast::<c_void>()),
                required,
                &mut required,
            )
        };
        unsafe {
            let _ = CloseHandle(token);
        }
        query
            .map_err(|error| appcontainer_error("SANDBOX_TOKEN_QUERY_FAILED", error.to_string()))?;

        let info = unsafe {
            ptr::read_unaligned(buffer.as_ptr().cast::<TOKEN_APPCONTAINER_INFORMATION>())
        };
        if info.TokenAppContainer.is_invalid() {
            return Err(appcontainer_error(
                "SANDBOX_TOKEN_QUERY_FAILED",
                "AppContainer token information did not contain a package SID.",
            ));
        }
        sid_string(info.TokenAppContainer)
    }

    fn grant_acl(path: &Path, sid: &str, rights: &str) -> Result<(), String> {
        let grant = format!("*{sid}:{rights}");
        let status = Command::new("icacls.exe")
            .arg(path)
            .args(["/grant", grant.as_str(), "/T", "/C", "/Q"])
            .status()
            .map_err(|error| error.to_string())?;
        if !status.success() {
            return Err(format!("icacls failed for {}: {status}", path.display()));
        }
        Ok(())
    }
}
