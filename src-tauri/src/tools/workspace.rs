use std::path::{Component, Path, PathBuf};

use serde_json::{json, Value};
use thiserror::Error;

use crate::workspace::{parse_wsl_path, ExecutionTarget, WslLocation};

pub const DEFAULT_EXCLUDED_NAMES: &[&str] = &[
    ".git",
    ".reference",
    "node_modules",
    "target",
    "dist",
    "build",
    ".venv",
    "venv",
    ".tox",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    "__pycache__",
];

#[derive(Debug, Clone)]
pub struct ResolvedPath {
    pub display: String,
    pub path: PathBuf,
    pub existed: bool,
}

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("{message}")]
    Tool {
        code: &'static str,
        message: String,
        category: &'static str,
        retryable: bool,
    },
    #[error("{message}")]
    ToolDetails {
        code: &'static str,
        message: String,
        category: &'static str,
        retryable: bool,
        details: Value,
    },
}

impl WorkspaceError {
    pub fn message(&self) -> String {
        match self {
            Self::Tool { message, .. } | Self::ToolDetails { message, .. } => message.clone(),
        }
    }

    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::Tool {
            code: "INVALID_ARGUMENT",
            message: message.into(),
            category: "validation",
            retryable: false,
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::Tool {
            code: "NOT_FOUND",
            message: message.into(),
            category: "not_found",
            retryable: false,
        }
    }

    pub fn absolute_path_denied() -> Self {
        Self::Tool {
            code: "ABSOLUTE_PATH_DENIED",
            message: "Absolute paths are denied.".into(),
            category: "security",
            retryable: false,
        }
    }

    pub fn path_outside_workspace() -> Self {
        Self::Tool {
            code: "PATH_OUTSIDE_WORKSPACE",
            message: "Path escapes the configured workspace.".into(),
            category: "security",
            retryable: false,
        }
    }

    pub fn symlink_escape() -> Self {
        Self::Tool {
            code: "SYMLINK_ESCAPE",
            message: "Path escapes the configured workspace.".into(),
            category: "security",
            retryable: false,
        }
    }

    pub fn not_a_directory(message: impl Into<String>) -> Self {
        Self::Tool {
            code: "NOT_A_DIRECTORY",
            message: message.into(),
            category: "validation",
            retryable: false,
        }
    }

    pub fn to_error_value(&self) -> Value {
        match self {
            Self::Tool {
                code,
                message,
                category,
                retryable,
            } => json!({
                "code": code,
                "message": message,
                "category": category,
                "retryable": retryable,
                "details": {}
            }),
            Self::ToolDetails {
                code,
                message,
                category,
                retryable,
                details,
            } => json!({
                "code": code,
                "message": message,
                "category": category,
                "retryable": retryable,
                "details": details
            }),
        }
    }
}

pub type WorkspaceResult<T> = Result<T, WorkspaceError>;

#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
    execution: ExecutionTarget,
}

impl Workspace {
    pub fn new(root: PathBuf) -> WorkspaceResult<Self> {
        let execution = ExecutionTarget::from_host_path(&root.to_string_lossy());
        Self::new_with_execution(root, execution)
    }

    pub fn new_with_execution(
        root: PathBuf,
        mut execution: ExecutionTarget,
    ) -> WorkspaceResult<Self> {
        if matches!(execution, ExecutionTarget::Host) {
            execution = ExecutionTarget::from_host_path(&root.to_string_lossy());
        }
        let root = root
            .canonicalize()
            .map_err(|_| WorkspaceError::invalid_argument("Workspace root must exist"))?;
        if !root.is_dir() {
            return Err(WorkspaceError::invalid_argument(
                "Workspace root must be a directory",
            ));
        }
        Ok(Self { root, execution })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn root_display(&self) -> String {
        self.root.to_string_lossy().into_owned()
    }

    pub fn execution_target(&self) -> &ExecutionTarget {
        &self.execution
    }

    pub fn wsl_location(&self) -> Option<WslLocation> {
        self.execution
            .wsl_location()
            .or_else(|| parse_wsl_path(&self.root))
    }

    pub fn is_wsl(&self) -> bool {
        self.wsl_location().is_some()
    }

    pub fn reject_unsafe_text(&self, raw_path: &str) -> WorkspaceResult<()> {
        if raw_path.is_empty() {
            return Err(WorkspaceError::invalid_argument(
                "Path must be a non-empty string",
            ));
        }
        if raw_path.contains('\0') {
            return Err(WorkspaceError::invalid_argument("Path contains a NUL byte"));
        }
        if raw_path.starts_with('/') || raw_path.starts_with('\\') {
            return Err(WorkspaceError::absolute_path_denied());
        }
        if raw_path.len() >= 2 {
            let bytes = raw_path.as_bytes();
            if bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
                return Err(WorkspaceError::absolute_path_denied());
            }
        }
        for part in Path::new(raw_path).components() {
            if matches!(part, Component::ParentDir) {
                return Err(WorkspaceError::path_outside_workspace());
            }
        }
        Ok(())
    }

    pub fn resolve_existing(&self, raw_path: &str) -> WorkspaceResult<ResolvedPath> {
        self.resolve_existing_at(&self.root, raw_path)
    }

    /// Resolve a read-only path inside the configured Workspace.
    /// Absolute paths, parent traversal, and symlink escapes are rejected.
    pub fn resolve_read_path(&self, raw_path: &str) -> WorkspaceResult<ResolvedPath> {
        self.resolve_existing(raw_path)
    }

    pub fn resolve_existing_at(
        &self,
        base: &Path,
        raw_path: &str,
    ) -> WorkspaceResult<ResolvedPath> {
        let raw = if raw_path.is_empty() { "." } else { raw_path };
        self.reject_unsafe_text(raw)?;
        let base = self.validate_base(base)?;
        let candidate = base.join(raw.replace('/', std::path::MAIN_SEPARATOR_STR));
        let resolved = candidate
            .canonicalize()
            .map_err(|_| WorkspaceError::not_found(format!("Path not found: {raw}")))?;
        self.ensure_inside_workspace(&candidate, &resolved)?;
        Ok(ResolvedPath {
            display: relative_display(&self.root, &resolved),
            path: resolved,
            existed: true,
        })
    }

    pub fn resolve_for_write(&self, raw_path: &str) -> WorkspaceResult<ResolvedPath> {
        self.reject_unsafe_text(raw_path)?;
        self.reject_protected_write_path(raw_path)?;
        let pure = Path::new(raw_path);
        if pure.file_name().is_none() || raw_path == "." || raw_path == ".." {
            return Err(WorkspaceError::invalid_argument("Invalid write target"));
        }
        let candidate = self
            .root
            .join(raw_path.replace('/', std::path::MAIN_SEPARATOR_STR));
        if candidate.exists() || candidate.is_symlink() {
            let resolved = candidate
                .canonicalize()
                .map_err(|_| WorkspaceError::not_found(format!("Path not found: {raw_path}")))?;
            self.ensure_inside_workspace(&candidate, &resolved)?;
            return Ok(ResolvedPath {
                display: relative_display(&self.root, &resolved),
                path: resolved,
                existed: true,
            });
        }
        let parent = candidate.parent().unwrap_or(&self.root);
        let resolved_parent = if parent.exists() {
            parent
                .canonicalize()
                .map_err(|_| WorkspaceError::not_found("Parent directory not found"))?
        } else {
            self.ensure_parent_chain(parent)?;
            parent.to_path_buf()
        };
        if !resolved_parent.starts_with(&self.root) {
            return Err(WorkspaceError::path_outside_workspace());
        }
        Ok(ResolvedPath {
            display: raw_path.replace('\\', "/"),
            path: candidate,
            existed: false,
        })
    }

    fn ensure_parent_chain(&self, parent: &Path) -> WorkspaceResult<()> {
        let mut cursor = parent;
        while !cursor.exists() {
            if cursor == self.root || cursor.parent() == Some(cursor) {
                break;
            }
            cursor = cursor.parent().unwrap_or(cursor);
        }
        if cursor.exists() {
            let resolved = cursor
                .canonicalize()
                .map_err(|_| WorkspaceError::not_found("Parent directory not found"))?;
            if !resolved.starts_with(&self.root) {
                return Err(WorkspaceError::path_outside_workspace());
            }
        }
        Ok(())
    }

    fn validate_base(&self, base: &Path) -> WorkspaceResult<PathBuf> {
        let resolved = base
            .canonicalize()
            .map_err(|_| WorkspaceError::not_found("Base path not found"))?;
        if !resolved.is_dir() {
            return Err(WorkspaceError::not_a_directory("Base is not a directory"));
        }
        if !resolved.starts_with(&self.root) {
            return Err(WorkspaceError::path_outside_workspace());
        }
        Ok(resolved)
    }

    fn ensure_inside_workspace(&self, candidate: &Path, resolved: &Path) -> WorkspaceResult<()> {
        if !resolved.starts_with(&self.root) {
            if candidate.is_symlink() {
                return Err(WorkspaceError::symlink_escape());
            }
            return Err(WorkspaceError::path_outside_workspace());
        }
        Ok(())
    }

    pub fn reject_write_symlink(&self, raw_path: &str) -> WorkspaceResult<()> {
        self.reject_unsafe_text(raw_path)?;
        let candidate = self
            .root
            .join(raw_path.replace('/', std::path::MAIN_SEPARATOR_STR));
        if candidate.is_symlink() {
            return Err(WorkspaceError::symlink_escape());
        }
        Ok(())
    }

    pub fn reject_protected_write_path(&self, raw_path: &str) -> WorkspaceResult<()> {
        let normalized = raw_path.replace('\\', "/");
        let first = normalized.split('/').next().unwrap_or("");
        if matches!(first, ".git" | ".github") {
            return Err(WorkspaceError::Tool {
                code: "PROTECTED_PATH",
                message: format!("禁止普通文件操作写入受保护目录: {raw_path}"),
                category: "security",
                retryable: false,
            });
        }
        Ok(())
    }

    pub fn is_ignored_path(
        &self,
        path: &Path,
        include_hidden: bool,
        include_ignored: bool,
        include_generated: bool,
    ) -> bool {
        self.is_ignored_scan_path(
            &self.root,
            path,
            include_hidden,
            include_ignored,
            include_generated,
        )
    }

    pub fn is_ignored_scan_path(
        &self,
        scan_root: &Path,
        path: &Path,
        include_hidden: bool,
        _include_ignored: bool,
        include_generated: bool,
    ) -> bool {
        let Ok(workspace_path) = path.strip_prefix(&self.root) else {
            return true;
        };
        let workspace_parts: Vec<String> = workspace_path
            .components()
            .filter_map(|part| match part {
                Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect();
        for part in &workspace_parts {
            if part.eq_ignore_ascii_case(".git") {
                return true;
            }
        }
        if !include_hidden {
            let hidden_path = path.strip_prefix(scan_root).unwrap_or(workspace_path);
            for part in hidden_path.components().filter_map(|part| match part {
                Component::Normal(name) => Some(name.to_string_lossy()),
                _ => None,
            }) {
                if part.starts_with('.') && part != "." {
                    return true;
                }
            }
        }
        if !include_generated {
            for part in &workspace_parts {
                if DEFAULT_EXCLUDED_NAMES.contains(&part.as_str()) {
                    return true;
                }
            }
        }
        false
    }

    pub fn is_safe_existing_path(&self, path: &Path) -> bool {
        path.canonicalize()
            .map(|p| p.starts_with(&self.root))
            .unwrap_or(false)
    }

    pub fn is_safe_read_path(&self, path: &Path) -> bool {
        path.exists() || path.is_symlink()
    }
}

#[cfg(windows)]
fn comparable_windows_display_path(path: &Path) -> String {
    let display = path.to_string_lossy().replace('\\', "/");
    if let Some(unc) = display
        .strip_prefix("//?/UNC/")
        .or_else(|| display.strip_prefix("//?/unc/"))
    {
        return format!("//{unc}");
    }
    display.strip_prefix("//?/").unwrap_or(&display).to_string()
}

pub fn relative_display(root: &Path, path: &Path) -> String {
    if let Ok(relative) = path.strip_prefix(root) {
        return relative.to_string_lossy().replace('\\', "/");
    }
    #[cfg(windows)]
    {
        let root_display = comparable_windows_display_path(root);
        let path_display = comparable_windows_display_path(path);
        if path_display.eq_ignore_ascii_case(&root_display) {
            return String::new();
        }
        let prefix = format!("{}/", root_display.trim_end_matches('/'));
        if path_display.len() >= prefix.len()
            && path_display[..prefix.len()].eq_ignore_ascii_case(&prefix)
        {
            return path_display[prefix.len()..].to_string();
        }
        return path_display;
    }
    #[cfg(not(windows))]
    path.to_string_lossy().replace('\\', "/")
}

pub fn tool_ok(mut value: Value) -> Value {
    if value.get("ok").is_none() {
        value
            .as_object_mut()
            .expect("tool result object")
            .insert("ok".into(), Value::Bool(true));
    }
    value
}

pub fn tool_err(error: WorkspaceError) -> Value {
    json!({
        "ok": false,
        "status": "error",
        "summary": error.message(),
        "error": error.to_error_value()
    })
}

pub fn tool_err_code(
    code: &'static str,
    message: impl Into<String>,
    category: &'static str,
) -> Value {
    let message = message.into();
    json!({
        "ok": false,
        "status": "error",
        "summary": message.clone(),
        "error": {
            "code": code,
            "message": message,
            "category": category,
            "retryable": false,
            "details": {}
        }
    })
}

pub fn wrap_tool_result(structured: Value) -> Value {
    wrap_mcp_tool_result("", &serde_json::json!({}), structured)
}

pub fn wrap_mcp_tool_result(tool_name: &str, args: &Value, structured: Value) -> Value {
    let is_error = structured.get("ok").and_then(Value::as_bool) == Some(false);
    let content = if matches!(tool_name, "view_image" | "desktop_screenshot")
        && args
            .get("output")
            .and_then(Value::as_str)
            .unwrap_or("mcp_image")
            == "mcp_image"
        && !is_error
    {
        vec![json!({
            "type": "image",
            "data": structured.get("base64").and_then(Value::as_str).unwrap_or(""),
            "mimeType": structured
                .get("mime_type")
                .and_then(Value::as_str)
                .unwrap_or("application/octet-stream")
        })]
    } else {
        vec![json!({
            "type": "text",
            "text": mcp_result_summary(tool_name, &structured, is_error)
        })]
    };
    json!({
        "content": content,
        "structuredContent": structured,
        "isError": is_error
    })
}

fn mcp_result_summary(tool_name: &str, structured: &Value, is_error: bool) -> String {
    if is_error {
        return structured
            .pointer("/error/message")
            .and_then(Value::as_str)
            .or_else(|| structured.get("summary").and_then(Value::as_str))
            .unwrap_or("Tool call failed.")
            .to_string();
    }
    // Chat guidance must also reach clients that render only MCP text content.
    if matches!(tool_name, "chat_open" | "chat_wait" | "chat_reply" | "chat_close") {
        if let Some(instruction) = structured.get("instruction").and_then(Value::as_str) {
            return instruction.to_string();
        }
    }
    if let Some(summary) = structured.get("summary").and_then(Value::as_str) {
        return summary.to_string();
    }
    if let Some(count) = structured.get("returned_count").and_then(Value::as_u64) {
        return format!("{tool_name} completed with {count} returned items.");
    }
    if let Some(count) = structured.get("total_matches").and_then(Value::as_u64) {
        return format!("{tool_name} completed with {count} matches.");
    }
    if let Some(count) = structured.get("commands_executed").and_then(Value::as_u64) {
        return format!("{tool_name} completed after executing {count} commands.");
    }
    if let Some(status) = structured.get("status").and_then(Value::as_str) {
        return format!("{tool_name} status: {status}.");
    }
    if tool_name.is_empty() {
        "Tool call completed successfully.".to_string()
    } else {
        format!("{tool_name} completed successfully.")
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use std::path::Path;

    use super::{relative_display, wrap_mcp_tool_result};

    #[cfg(windows)]
    #[test]
    fn relative_display_matches_verbatim_and_normal_windows_paths() {
        let root = Path::new(r"\\?\C:\Users\Tester\workspace");
        let normal = Path::new(r"C:\Users\Tester\workspace\nested-repo\tracked.txt");
        assert_eq!(relative_display(root, normal), "nested-repo/tracked.txt");

        let unc_root = Path::new(r"\\?\UNC\server\share\workspace");
        let unc_path = Path::new(r"\\server\share\workspace\nested\file.txt");
        assert_eq!(relative_display(unc_root, unc_path), "nested/file.txt");
    }

    #[test]
    fn mcp_text_content_is_a_summary_not_a_second_structured_payload() {
        let wrapped = wrap_mcp_tool_result(
            "read_file",
            &json!({}),
            json!({"ok": true, "path": "large.txt", "content": "x".repeat(32 * 1024)}),
        );

        let text = wrapped["content"][0]["text"]
            .as_str()
            .expect("summary text");
        assert!(text.len() < 128, "{text}");
        assert!(!text.contains(&"x".repeat(128)));
        assert_eq!(
            wrapped["structuredContent"]["content"]
                .as_str()
                .expect("structured content")
                .len(),
            32 * 1024
        );
    }
}
