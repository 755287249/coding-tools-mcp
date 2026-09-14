use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use regex::Regex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::platform::wsl::std_command_for_workspace_with_env;
use crate::tools::workspace::{relative_display, tool_ok, Workspace, WorkspaceError};

#[derive(Debug, Clone)]
struct GitTarget {
    repo_path: String,
    root: PathBuf,
    git_dir: String,
    common_dir: String,
    branch: String,
    head: String,
    fingerprint: String,
}

impl GitTarget {
    fn metadata(&self) -> Value {
        json!({
            "repo_path": self.repo_path,
            "repo_root": self.root.to_string_lossy(),
            "git_dir": self.git_dir,
            "git_common_dir": self.common_dir,
            "branch": self.branch,
            "head": self.head,
            "repo_fingerprint": self.fingerprint,
        })
    }
}

#[derive(Debug, Clone, Default)]
struct GitWorktreeEntry {
    path: String,
    absolute_path: PathBuf,
    head: String,
    branch: String,
    detached: bool,
    bare: bool,
    locked: Option<String>,
    prunable: Option<String>,
    primary: bool,
    inside_workspace: bool,
    managed: bool,
    removable: bool,
    dirty: Option<bool>,
    branch_merged: Option<bool>,
}

impl GitWorktreeEntry {
    fn value(&self) -> Value {
        json!({
            "path": self.path,
            "head": self.head,
            "branch": self.branch,
            "detached": self.detached,
            "bare": self.bare,
            "locked": self.locked.as_ref().map(|value| if value.is_empty() { Value::Bool(true) } else { Value::String(value.clone()) }).unwrap_or(Value::Null),
            "prunable": self.prunable.as_ref().map(|value| if value.is_empty() { Value::Bool(true) } else { Value::String(value.clone()) }).unwrap_or(Value::Null),
            "primary": self.primary,
            "inside_workspace": self.inside_workspace,
            "managed": self.managed,
            "removable": self.removable,
            "dirty": self.dirty,
            "branch_merged": self.branch_merged,
        })
    }
}

pub fn git_status(ws: &Workspace, args: &Value) -> Result<Value, WorkspaceError> {
    let path = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let max_entries = args
        .get("max_entries")
        .and_then(Value::as_u64)
        .unwrap_or(1000) as usize;
    let include_untracked = args
        .get("include_untracked")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let target = match resolve_git_target(ws, &json!({"repo_path": path})) {
        Ok(target) => target,
        Err(WorkspaceError::Tool {
            code: "NOT_GIT_REPOSITORY",
            message,
            ..
        }) => {
            return Ok(tool_ok(json!({
                "is_repo": false,
                "clean": true,
                "entries": [],
                "warnings": [message]
            })))
        }
        Err(error) => return Err(error),
    };

    let mut status_args = vec!["status", "--porcelain=v1", "-b"];
    if !include_untracked {
        status_args.push("--untracked-files=no");
    }
    let completed = run_git(&target.root, &status_args, Duration::from_secs(10))?;
    if !completed.success && completed.exit_code != 0 {
        return Err(git_error(&completed.stderr));
    }

    let mut branch = String::new();
    let mut upstream = String::new();
    let mut ahead = 0i64;
    let mut behind = 0i64;
    let mut entries = Vec::new();
    let lines: Vec<_> = completed.stdout.lines().collect();
    let total_lines = lines.len();

    for line in lines {
        if let Some(rest) = line.strip_prefix("## ") {
            (branch, upstream, ahead, behind) = parse_branch_line(rest);
            continue;
        }
        if line.len() < 4 {
            continue;
        }
        let index_status = line.chars().next().unwrap_or(' ').to_string();
        let worktree_status = line.chars().nth(1).unwrap_or(' ').to_string();
        let mut path_text = line[3..].to_string();
        let original = if let Some((orig, new)) = path_text.split_once(" -> ") {
            let orig = orig.to_string();
            path_text = new.to_string();
            Some(orig)
        } else {
            None
        };
        let mut entry = json!({
            "path": path_text,
            "index_status": index_status,
            "worktree_status": worktree_status
        });
        if let Some(orig) = original {
            entry["original_path"] = json!(orig);
        }
        entries.push(entry);
        if entries.len() >= max_entries {
            break;
        }
    }

    Ok(tool_ok(json!({
        "is_repo": true,
        "branch": branch,
        "head": target.head,
        "upstream": upstream,
        "ahead": ahead,
        "behind": behind,
        "clean": entries.is_empty(),
        "repo": target.metadata(),
        "repo_fingerprint": target.fingerprint,
        "entries": entries,
        "truncated": entries.len() >= max_entries && total_lines > max_entries + 1,
        "warnings": []
    })))
}

pub fn git_diff(ws: &Workspace, args: &Value) -> Result<Value, WorkspaceError> {
    let target = resolve_git_target(ws, args)?;
    let staged = args.get("staged").and_then(Value::as_bool).unwrap_or(false);
    let unstaged = args
        .get("unstaged")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let requested_context = args
        .get("context_lines")
        .and_then(Value::as_u64)
        .unwrap_or(3);
    let context = requested_context.min(20);
    let max_bytes = args
        .get("max_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(262_144) as usize;

    let path_filters = git_read_paths(ws, &target, args)?;

    if !is_git_repo(&target.root) {
        return Ok(tool_ok(json!({
            "diff": "",
            "files": [],
            "truncated": false,
            "warnings": ["not a git repository"]
        })));
    }

    let mut chunks = Vec::new();
    if unstaged {
        chunks.push(run_git_diff(&target.root, context, &path_filters, false)?);
    }
    if staged {
        chunks.push(run_git_diff(&target.root, context, &path_filters, true)?);
    }
    let mut combined = chunks.join("\n");
    if !combined.is_empty() && !combined.ends_with('\n') {
        combined.push('\n');
    }
    let truncated = combined.len() > max_bytes;
    let diff_text = if truncated {
        String::from_utf8_lossy(&combined.as_bytes()[..max_bytes]).into_owned()
    } else {
        combined
    };
    let files = parse_diff_files(&diff_text);
    Ok(tool_ok(json!({
        "diff": diff_text,
        "files": files,
        "repo": target.metadata(),
        "repo_fingerprint": target.fingerprint,
        "paths": path_filters,
        "arguments_normalized": requested_context != context,
        "normalized_arguments": if requested_context != context {
            json!({ "context_lines": context })
        } else {
            Value::Null
        },
        "truncated": truncated,
        "warnings": if truncated { vec!["diff truncated"] } else { vec![] }
    })))
}

pub fn git_log(ws: &Workspace, args: &Value) -> Result<Value, WorkspaceError> {
    let path = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let resolved = ws.resolve_existing(path)?;
    let ref_name = validate_git_ref(args.get("ref").and_then(Value::as_str).unwrap_or("HEAD"))?;
    let max_count = args
        .get("max_count")
        .and_then(Value::as_u64)
        .unwrap_or(20)
        .clamp(1, 100) as usize;
    let skip = args
        .get("skip")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(10_000) as usize;

    if !is_git_repo(ws.root()) {
        return Ok(tool_ok(json!({
            "is_repo": false,
            "commits": [],
            "truncated": false,
            "warnings": []
        })));
    }

    let max_count_arg = format!("--max-count={}", max_count + 1);
    let skip_arg = format!("--skip={skip}");
    let pretty = "--pretty=format:%H%x1f%h%x1f%an%x1f%ae%x1f%ad%x1f%s%x1e";
    let path_filter = if resolved.display.is_empty() {
        ".".to_string()
    } else {
        resolved.display.clone()
    };
    let mut cmd_args = vec![
        "log",
        max_count_arg.as_str(),
        skip_arg.as_str(),
        "--date=iso-strict",
        pretty,
        ref_name,
    ];
    if path_filter != "." {
        cmd_args.push("--");
        cmd_args.push(path_filter.as_str());
    }

    let completed = run_git(ws.root(), &cmd_args, Duration::from_secs(10))?;
    if !completed.success {
        return Err(git_error(&completed.stderr));
    }

    let mut commits = Vec::new();
    for record in completed.stdout.split('\u{1e}') {
        let fields: Vec<String> = record
            .trim()
            .split('\u{1f}')
            .map(str::trim)
            .map(str::to_string)
            .collect();
        if fields.len() < 6 || fields[0].is_empty() {
            continue;
        }
        commits.push(json!({
            "hash": fields[0],
            "short_hash": fields[1],
            "author_name": fields[2],
            "author_email": fields[3],
            "author_date": fields[4],
            "subject": fields[5],
        }));
    }
    let truncated = commits.len() > max_count;
    Ok(tool_ok(json!({
        "is_repo": true,
        "ref": ref_name,
        "path": path_filter,
        "commits": commits.into_iter().take(max_count).collect::<Vec<_>>(),
        "truncated": truncated,
        "warnings": if truncated { vec!["commit limit reached"] } else { Vec::<&str>::new() }
    })))
}

pub fn git_show(ws: &Workspace, args: &Value) -> Result<Value, WorkspaceError> {
    let target = resolve_git_target(ws, args)?;
    if !is_git_repo(&target.root) {
        return Ok(tool_ok(json!({
            "is_repo": false,
            "content": "",
            "files": [],
            "truncated": false,
            "warnings": []
        })));
    }

    let rev = validate_git_ref(args.get("rev").and_then(Value::as_str).unwrap_or("HEAD"))?;
    let requested_context = args
        .get("context_lines")
        .and_then(Value::as_u64)
        .unwrap_or(3);
    let context = requested_context.min(20);
    let max_bytes = args
        .get("max_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(262_144) as usize;
    let include_diff = args
        .get("include_diff")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let path_filters = git_read_paths(ws, &target, args)?;

    let unified = format!("--unified={context}");
    let mut cmd_args = vec!["show", "--no-ext-diff", "--format=fuller", unified.as_str()];
    if !include_diff {
        cmd_args.push("--no-patch");
    }
    cmd_args.push(rev);
    if !path_filters.is_empty() {
        cmd_args.push("--");
        for p in &path_filters {
            cmd_args.push(p.as_str());
        }
    }

    let completed = run_git(&target.root, &cmd_args, Duration::from_secs(10))?;
    if !completed.success {
        return Err(git_error(&completed.stderr));
    }

    let truncated = completed.stdout.len() > max_bytes;
    let content = if truncated {
        String::from_utf8_lossy(&completed.stdout.as_bytes()[..max_bytes]).into_owned()
    } else {
        completed.stdout.clone()
    };
    let files = parse_diff_files(&content);
    Ok(tool_ok(json!({
        "is_repo": true,
        "rev": rev,
        "content": content,
        "files": files,
        "repo": target.metadata(),
        "repo_fingerprint": target.fingerprint,
        "paths": path_filters,
        "arguments_normalized": requested_context != context,
        "normalized_arguments": if requested_context != context {
            json!({ "context_lines": context })
        } else {
            Value::Null
        },
        "truncated": truncated,
        "output_bytes": content.len(),
        "warnings": if truncated { vec!["output truncated"] } else { Vec::<&str>::new() }
    })))
}

pub fn git_blame(ws: &Workspace, args: &Value) -> Result<Value, WorkspaceError> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| WorkspaceError::invalid_argument("path is required"))?;
    let resolved = ws.resolve_existing(path)?;
    if resolved.path.is_dir() {
        return Err(WorkspaceError::Tool {
            code: "IS_DIRECTORY",
            message: "Path is a directory.".into(),
            category: "validation",
            retryable: false,
        });
    }
    if !is_git_repo(ws.root()) {
        return Ok(tool_ok(json!({
            "is_repo": false,
            "path": resolved.display,
            "lines": [],
            "truncated": false,
            "warnings": []
        })));
    }

    let ref_arg = args.get("rev").and_then(Value::as_str);
    let git_ref = ref_arg.map(validate_git_ref).transpose()?;
    let start_line = args
        .get("start_line")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1) as usize;
    let end_line_arg = args
        .get("end_line")
        .and_then(Value::as_u64)
        .map(|v| v as usize);
    let max_lines = args
        .get("max_lines")
        .and_then(Value::as_u64)
        .unwrap_or(200)
        .clamp(1, 1000) as usize;

    let final_line = match end_line_arg {
        None => start_line + max_lines - 1,
        Some(end) if end < start_line => {
            return Err(WorkspaceError::invalid_argument(
                "end_line must be >= start_line.",
            ));
        }
        Some(end) => end,
    };
    let requested_lines = final_line - start_line + 1;
    let mut truncated = requested_lines > max_lines;
    let final_line = final_line.min(start_line + max_lines - 1);

    let line_range = format!("{start_line},{final_line}");
    let mut cmd_args = vec!["blame", "--line-porcelain", "-L", line_range.as_str()];
    if let Some(r) = git_ref {
        cmd_args.push(r);
    }
    cmd_args.push("--");
    cmd_args.push(resolved.display.as_str());

    let completed = run_git(ws.root(), &cmd_args, Duration::from_secs(10))?;
    if !completed.success {
        return Err(git_error(&completed.stderr));
    }

    let mut lines = parse_git_blame_porcelain(&completed.stdout);
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        truncated = true;
    }

    Ok(tool_ok(json!({
        "is_repo": true,
        "path": resolved.display,
        "rev": ref_arg,
        "start_line": start_line,
        "end_line": final_line,
        "lines": lines,
        "truncated": truncated,
        "warnings": if truncated { vec!["line limit reached"] } else { Vec::<&str>::new() }
    })))
}

pub fn git_branch(ws: &Workspace, args: &Value) -> Result<Value, WorkspaceError> {
    let target = resolve_git_target(ws, args)?;
    verify_expected_head(&target, args)?;
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| WorkspaceError::invalid_argument("action is required"))?;
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| WorkspaceError::invalid_argument("name is required"))?;
    validate_branch_name(&target.root, name)?;
    let dry_run = args
        .get("dry_run")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let force = args.get("force").and_then(Value::as_bool).unwrap_or(false);
    let mut command = vec!["git".to_string()];
    let mut git_args = Vec::<String>::new();
    match action {
        "create" => {
            let switch = args.get("switch").and_then(Value::as_bool).unwrap_or(true);
            if switch {
                git_args.extend(["switch".into(), "-c".into(), name.into()]);
            } else {
                git_args.extend(["branch".into(), name.into()]);
            }
            if let Some(start) = args.get("start_point").and_then(Value::as_str) {
                git_args.push(validate_git_ref(start)?.to_string());
            }
        }
        "switch" => {
            git_args.extend(["switch".into(), name.into()]);
        }
        "delete" => {
            if !args
                .get("confirm")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                return Err(WorkspaceError::ToolDetails {
                    code: "DANGEROUS_OPERATION_REQUIRES_CONFIRMATION",
                    message: "Deleting a branch requires confirm=true".into(),
                    category: "permission",
                    retryable: false,
                    details: json!({"action": action, "name": name}),
                });
            }
            git_args.extend([
                "branch".into(),
                if force { "-D".into() } else { "-d".into() },
                name.into(),
            ]);
        }
        _ => {
            return Err(WorkspaceError::invalid_argument(
                "action must be create, switch, or delete",
            ))
        }
    }
    command.extend(git_args.clone());
    if dry_run {
        return Ok(tool_ok(json!({
            "dry_run": true,
            "applied": false,
            "action": action,
            "name": name,
            "command": command,
            "repo": target.metadata(),
            "warnings": []
        })));
    }
    let completed = run_git_strings(&target.root, &git_args, Duration::from_secs(15))?;
    if !completed.success {
        return Err(git_error(&completed.stderr));
    }
    let status = git_status(ws, &json!({"path": target.repo_path}))?;
    Ok(tool_ok(json!({
        "dry_run": false,
        "applied": true,
        "action": action,
        "name": name,
        "command": command,
        "stdout": completed.stdout.trim(),
        "repo": target.metadata(),
        "status": status,
        "affected_files": [],
        "warnings": []
    })))
}

fn git_path_identity(path: &Path) -> String {
    let value = git_cli_path(path);
    #[cfg(windows)]
    return value
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase();
    #[cfg(not(windows))]
    value.trim_end_matches('/').to_string()
}

fn git_worktree_display(ws: &Workspace, path: &Path) -> String {
    let workspace = PathBuf::from(git_cli_path(ws.root()));
    let candidate = PathBuf::from(git_cli_path(path));
    candidate
        .strip_prefix(&workspace)
        .ok()
        .map(|relative| {
            if relative.as_os_str().is_empty() {
                ".".to_string()
            } else {
                relative.to_string_lossy().replace('\\', "/")
            }
        })
        .unwrap_or_else(|| candidate.to_string_lossy().replace('\\', "/"))
}

fn list_git_worktrees(
    ws: &Workspace,
    root: &Path,
) -> Result<Vec<GitWorktreeEntry>, WorkspaceError> {
    let completed = run_git(
        root,
        &["worktree", "list", "--porcelain"],
        Duration::from_secs(10),
    )?;
    if !completed.success {
        return Err(git_error(&completed.stderr));
    }
    let mut entries = Vec::new();
    let mut current: Option<GitWorktreeEntry> = None;
    let flush = |current: &mut Option<GitWorktreeEntry>, entries: &mut Vec<GitWorktreeEntry>| {
        if let Some(entry) = current.take() {
            entries.push(entry);
        }
    };
    for line in completed.stdout.lines() {
        if line.is_empty() {
            flush(&mut current, &mut entries);
            continue;
        }
        let (field, value) = line.split_once(' ').unwrap_or((line, ""));
        if field == "worktree" {
            flush(&mut current, &mut entries);
            let absolute_path = PathBuf::from(value);
            current = Some(GitWorktreeEntry {
                path: git_worktree_display(ws, &absolute_path),
                absolute_path,
                ..Default::default()
            });
            continue;
        }
        let Some(entry) = current.as_mut() else {
            continue;
        };
        match field {
            "HEAD" => entry.head = value.to_string(),
            "branch" => {
                entry.branch = value
                    .strip_prefix("refs/heads/")
                    .unwrap_or(value)
                    .to_string()
            }
            "detached" => entry.detached = true,
            "bare" => entry.bare = true,
            "locked" => entry.locked = Some(value.to_string()),
            "prunable" => entry.prunable = Some(value.to_string()),
            _ => {}
        }
    }
    flush(&mut current, &mut entries);

    let merged = run_git(
        root,
        &["branch", "--merged", "HEAD", "--format=%(refname:short)"],
        Duration::from_secs(10),
    )?;
    let merged_branches = if merged.success {
        merged
            .stdout
            .lines()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect::<std::collections::HashSet<_>>()
    } else {
        std::collections::HashSet::new()
    };
    let workspace_identity = git_path_identity(ws.root());
    let workspace_prefix = format!("{}{}", workspace_identity, std::path::MAIN_SEPARATOR);
    for entry in &mut entries {
        let identity = git_path_identity(&entry.absolute_path);
        entry.primary = identity == workspace_identity;
        entry.inside_workspace = entry.primary || identity.starts_with(&workspace_prefix);
        entry.managed =
            entry.inside_workspace && !entry.primary && entry.path.starts_with(".worktrees/");
        entry.removable = entry.inside_workspace && !entry.primary;
        entry.branch_merged = if entry.branch.is_empty() {
            None
        } else {
            Some(merged_branches.contains(&entry.branch))
        };
        entry.dirty = if entry.inside_workspace && !entry.bare && entry.absolute_path.exists() {
            let status = run_git(
                &entry.absolute_path,
                &["status", "--porcelain=v1", "--untracked-files=normal"],
                Duration::from_secs(10),
            )?;
            status.success.then(|| !status.stdout.trim().is_empty())
        } else {
            None
        };
    }
    Ok(entries)
}

fn worktree_branch_slug(branch: &str) -> String {
    let mut slug = String::new();
    let mut pending_dash = false;
    for ch in branch.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            if pending_dash && !slug.is_empty() {
                slug.push('-');
            }
            pending_dash = false;
            slug.push(ch);
        } else {
            pending_dash = true;
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "worktree".to_string()
    } else {
        slug.to_string()
    }
}

fn local_branch_exists(root: &Path, branch: &str) -> Result<bool, WorkspaceError> {
    let reference = format!("refs/heads/{branch}");
    let completed = run_git(
        root,
        &["show-ref", "--verify", "--quiet", reference.as_str()],
        Duration::from_secs(10),
    )?;
    Ok(completed.success)
}

fn git_cli_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    #[cfg(windows)]
    {
        if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{rest}");
        }
        return raw.strip_prefix(r"\\?\").unwrap_or(&raw).to_string();
    }
    #[cfg(not(windows))]
    raw.into_owned()
}

fn worktree_tool_error(
    code: &'static str,
    message: impl Into<String>,
    category: &'static str,
) -> WorkspaceError {
    WorkspaceError::ToolDetails {
        code,
        message: message.into(),
        category,
        retryable: false,
        details: json!({}),
    }
}

pub fn git_worktree(ws: &Workspace, args: &Value) -> Result<Value, WorkspaceError> {
    let target = resolve_git_target(ws, args)?;
    verify_expected_head(&target, args)?;
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| WorkspaceError::invalid_argument("action is required"))?;
    let dry_run = args
        .get("dry_run")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let force = args.get("force").and_then(Value::as_bool).unwrap_or(false);

    if action == "list" {
        let worktrees = list_git_worktrees(ws, &target.root)?
            .iter()
            .map(GitWorktreeEntry::value)
            .collect::<Vec<_>>();
        return Ok(tool_ok(json!({
            "action": action,
            "applied": false,
            "repo": target.metadata(),
            "worktrees": worktrees,
        })));
    }
    if !matches!(action, "create" | "remove") {
        return Err(WorkspaceError::invalid_argument(
            "action must be list, create, or remove",
        ));
    }
    if action == "create" {
        let branch = args
            .get("branch")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| WorkspaceError::invalid_argument("branch is required for create"))?;
        validate_branch_name(&target.root, branch)?;
        let default_path = format!(".worktrees/{}", worktree_branch_slug(branch));
        let requested_path = args
            .get("path")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(default_path.as_str());
        let resolved = ws.resolve_for_write(requested_path)?;
        if resolved.existed {
            return Err(worktree_tool_error(
                "GIT_WORKTREE_PATH_EXISTS",
                format!("Worktree path already exists: {}", resolved.display),
                "git",
            ));
        }
        let start_point = validate_git_ref(
            args.get("start_point")
                .and_then(Value::as_str)
                .unwrap_or("HEAD"),
        )?;
        let configured_mode = args
            .get("branch_mode")
            .and_then(Value::as_str)
            .unwrap_or("auto");
        if !matches!(configured_mode, "auto" | "create" | "existing") {
            return Err(WorkspaceError::invalid_argument(
                "branch_mode must be auto, create, or existing",
            ));
        }
        let branch_exists = local_branch_exists(&target.root, branch)?;
        let branch_mode = match configured_mode {
            "auto" if branch_exists => "existing",
            "auto" => "create",
            other => other,
        };
        if branch_mode == "create" && branch_exists {
            return Err(worktree_tool_error(
                "GIT_WORKTREE_BRANCH_EXISTS",
                format!("Branch already exists: {branch}"),
                "conflict",
            ));
        }
        if branch_mode == "existing" && !branch_exists {
            return Err(worktree_tool_error(
                "GIT_WORKTREE_BRANCH_NOT_FOUND",
                format!("Branch does not exist: {branch}"),
                "conflict",
            ));
        }
        let git_args = if branch_mode == "existing" {
            vec![
                "worktree".to_string(),
                "add".to_string(),
                git_cli_path(&resolved.path),
                branch.to_string(),
            ]
        } else {
            vec![
                "worktree".to_string(),
                "add".to_string(),
                "-b".to_string(),
                branch.to_string(),
                git_cli_path(&resolved.path),
                start_point.to_string(),
            ]
        };
        if dry_run {
            return Ok(tool_ok(json!({
                "action": action,
                "dry_run": true,
                "applied": false,
                "path": resolved.display,
                "branch": branch,
                "branch_mode": branch_mode,
                "start_point": start_point,
                "command": std::iter::once("git".to_string()).chain(git_args.clone()).collect::<Vec<_>>(),
                "repo": target.metadata(),
            })));
        }
        if let Some(parent) = resolved.path.parent() {
            fs::create_dir_all(parent).map_err(|error| WorkspaceError::Tool {
                code: "FILE_WRITE_FAILED",
                message: format!("failed to create worktree parent directory: {error}"),
                category: "filesystem",
                retryable: false,
            })?;
        }
        let completed = run_git_strings(&target.root, &git_args, Duration::from_secs(60))?;
        if !completed.success {
            return Err(git_error(&completed.stderr));
        }
        let worktrees = list_git_worktrees(ws, &target.root)?
            .iter()
            .map(GitWorktreeEntry::value)
            .collect::<Vec<_>>();
        return Ok(tool_ok(json!({
            "action": action,
            "applied": true,
            "path": resolved.display,
            "branch": branch,
            "branch_mode": branch_mode,
            "start_point": start_point,
            "repo": target.metadata(),
            "worktrees": worktrees,
        })));
    }

    let requested_path = args
        .get("path")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| WorkspaceError::invalid_argument("path is required for remove"))?;
    let resolved = ws.resolve_existing(requested_path)?;
    if resolved.path == ws.root() {
        return Err(worktree_tool_error(
            "GIT_WORKTREE_PRIMARY_REMOVE_DENIED",
            "The configured workspace root cannot be removed as a linked worktree",
            "permission",
        ));
    }
    let entries = list_git_worktrees(ws, &target.root)?;
    let entry = entries
        .iter()
        .find(|entry| {
            git_path_identity(&entry.absolute_path) == git_path_identity(&resolved.path)
                || entry.path == resolved.display
        })
        .cloned()
        .ok_or_else(|| {
            worktree_tool_error(
                "GIT_WORKTREE_NOT_FOUND",
                format!("Linked worktree is not registered: {}", resolved.display),
                "git",
            )
        })?;
    if entry.locked.is_some() && !force {
        return Err(worktree_tool_error(
            "GIT_WORKTREE_LOCKED",
            format!("Linked worktree is locked: {}", resolved.display),
            "git",
        ));
    }
    if !force {
        let status = run_git(
            &resolved.path,
            &["status", "--porcelain=v1", "--untracked-files=normal"],
            Duration::from_secs(10),
        )?;
        if !status.success {
            return Err(git_error(&status.stderr));
        }
        if !status.stdout.trim().is_empty() {
            return Err(worktree_tool_error(
                "GIT_WORKTREE_DIRTY",
                format!(
                    "Linked worktree has uncommitted changes: {}",
                    resolved.display
                ),
                "conflict",
            ));
        }
    }
    if dry_run {
        return Ok(tool_ok(json!({
            "action": action,
            "dry_run": true,
            "applied": false,
            "path": resolved.display,
            "branch": if entry.branch.is_empty() { Value::Null } else { Value::String(entry.branch) },
            "repo": target.metadata(),
        })));
    }
    if !args
        .get("confirm")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(worktree_tool_error(
            "DANGEROUS_OPERATION_REQUIRES_CONFIRMATION",
            "Removing a linked worktree requires confirm=true",
            "permission",
        ));
    }
    let mut remove_args = vec!["worktree".to_string(), "remove".to_string()];
    if force {
        remove_args.push("--force".to_string());
    }
    remove_args.push(git_cli_path(&resolved.path));
    let removed = run_git_strings(&target.root, &remove_args, Duration::from_secs(60))?;
    let mut warnings = Vec::<String>::new();
    if !removed.success {
        let remaining = list_git_worktrees(ws, &target.root)?;
        let still_registered = remaining.iter().any(|candidate| {
            git_path_identity(&candidate.absolute_path) == git_path_identity(&resolved.path)
        });
        if still_registered {
            return Err(git_error(&removed.stderr));
        }
        warnings.push(format!(
            "worktree registration removed but Git reported cleanup failure: {}",
            if removed.stderr.trim().is_empty() {
                removed.stdout.trim()
            } else {
                removed.stderr.trim()
            }
        ));
    }

    if resolved.path.exists() {
        let mut cleanup_error = None;
        for _ in 0..3 {
            match fs::remove_dir_all(&resolved.path) {
                Ok(()) => {
                    cleanup_error = None;
                    break;
                }
                Err(error) => {
                    cleanup_error = Some(error.to_string());
                    thread::sleep(Duration::from_millis(50));
                }
            }
        }
        if let Some(error) = cleanup_error {
            warnings.push(format!("worktree directory cleanup incomplete: {error}"));
        }
    }

    let mut branch_deleted = false;
    if args
        .get("delete_branch")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && !entry.branch.is_empty()
    {
        let delete_args = vec![
            "branch".to_string(),
            if force {
                "-D".to_string()
            } else {
                "-d".to_string()
            },
            entry.branch.clone(),
        ];
        let deleted = run_git_strings(&target.root, &delete_args, Duration::from_secs(30))?;
        if deleted.success {
            branch_deleted = true;
        } else {
            warnings.push(format!(
                "worktree removed but branch deletion failed: {}",
                deleted.stderr.trim()
            ));
        }
    }
    let worktrees = list_git_worktrees(ws, &target.root)?
        .iter()
        .map(GitWorktreeEntry::value)
        .collect::<Vec<_>>();
    Ok(tool_ok(json!({
        "action": action,
        "applied": true,
        "path": resolved.display,
        "branch": if entry.branch.is_empty() { Value::Null } else { Value::String(entry.branch) },
        "branch_deleted": branch_deleted,
        "repo": target.metadata(),
        "worktrees": worktrees,
        "warnings": warnings,
    })))
}

pub fn git_stage(ws: &Workspace, args: &Value) -> Result<Value, WorkspaceError> {
    prevalidate_git_paths(ws, args)?;
    let target = resolve_git_target(ws, args)?;
    verify_expected_head(&target, args)?;
    let all = args.get("all").and_then(Value::as_bool).unwrap_or(false);
    let paths = git_paths(ws, &target, args)?;
    if !all && paths.is_empty() {
        return Err(WorkspaceError::invalid_argument(
            "paths is required unless all=true",
        ));
    }
    let mut git_args = vec!["add".to_string()];
    if all {
        git_args.push("-A".into());
    } else {
        git_args.push("--".into());
        git_args.extend(paths.clone());
    }
    if args
        .get("dry_run")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Ok(tool_ok(json!({
            "dry_run": true,
            "applied": false,
            "paths": paths,
            "all": all,
            "command": std::iter::once("git".to_string()).chain(git_args).collect::<Vec<_>>(),
            "repo": target.metadata(),
            "warnings": []
        })));
    }
    let completed = run_git_strings(&target.root, &git_args, Duration::from_secs(20))?;
    if !completed.success {
        return Err(git_error(&completed.stderr));
    }
    let status = git_status(ws, &json!({"path": target.repo_path}))?;
    Ok(tool_ok(json!({
        "dry_run": false,
        "applied": true,
        "paths": paths,
        "all": all,
        "repo": target.metadata(),
        "status": status,
        "affected_files": paths.iter().map(|path| json!({"path": path, "operation": "stage"})).collect::<Vec<_>>(),
        "warnings": []
    })))
}

pub fn git_commit(ws: &Workspace, args: &Value) -> Result<Value, WorkspaceError> {
    prevalidate_git_paths(ws, args)?;
    let target = resolve_git_target(ws, args)?;
    verify_expected_head(&target, args)?;
    let message = args
        .get("message")
        .and_then(Value::as_str)
        .ok_or_else(|| WorkspaceError::invalid_argument("message is required"))?;
    if message.trim().is_empty() || message.len() > 10_000 {
        return Err(WorkspaceError::invalid_argument(
            "message must be between 1 and 10000 characters",
        ));
    }
    let paths = git_paths(ws, &target, args)?;
    let all = args.get("all").and_then(Value::as_bool).unwrap_or(false);
    let allow_empty = args
        .get("allow_empty")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let require_clean_index = args
        .get("require_clean_index_before")
        .and_then(Value::as_bool)
        .unwrap_or(!paths.is_empty() || all);
    let expected_head = args.get("expected_head").and_then(Value::as_str);
    let actual_head_before =
        git_rev_parse(&target.root, "HEAD").unwrap_or_else(|| target.head.clone());
    let index_clean = git_index_clean(&target.root)?;
    let staged_paths_before = git_staged_paths(&target.root)?;
    if require_clean_index && !index_clean {
        return Err(WorkspaceError::ToolDetails {
            code: "GIT_INDEX_NOT_CLEAN",
            message: "git_commit requires a clean index before staging paths".into(),
            category: "conflict",
            retryable: false,
            details: json!({
                "transaction_stage": "preflight_index",
                "expected_head": expected_head,
                "actual_head_before": actual_head_before,
                "selected_path_count": paths.len(),
                "staged_path_count_before": staged_paths_before.len(),
                "index_clean_before": index_clean,
                "staged_by_tool": !paths.is_empty() || all,
                "suggestion": "commit or unstage existing staged changes, or set require_clean_index_before=false"
            }),
        });
    }
    if args
        .get("dry_run")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Ok(tool_ok(json!({
            "dry_run": true,
            "applied": false,
            "message": message,
            "paths": paths,
            "all": all,
            "index_clean": index_clean,
            "index_clean_before": index_clean,
            "staged_path_count_before": staged_paths_before.len(),
            "selected_path_count": paths.len(),
            "staged_by_tool": !paths.is_empty() || all,
            "expected_head": expected_head,
            "actual_head_before": actual_head_before,
            "transaction_stage": "preflight",
            "would_stage": !paths.is_empty() || all,
            "would_commit": true,
            "repo": target.metadata(),
            "warnings": []
        })));
    }

    let old_head = actual_head_before.clone();
    let staged_by_tool = !paths.is_empty() || all;
    if staged_by_tool {
        let mut add_args = vec!["add".to_string()];
        if all {
            add_args.push("-A".into());
        } else {
            add_args.push("--".into());
            add_args.extend(paths.clone());
        }
        let staged = run_git_strings(&target.root, &add_args, Duration::from_secs(20))?;
        if !staged.success {
            let index_restored = restore_clean_index_after_git_transaction_failure(
                &target.root,
                staged_by_tool,
                index_clean,
            );
            return Err(WorkspaceError::ToolDetails {
                code: "GIT_STAGE_FAILED",
                message: staged.stderr.trim().to_string(),
                category: "runtime",
                retryable: false,
                details: json!({
                    "transaction_stage": "stage",
                    "expected_head": expected_head,
                    "actual_head_before": old_head,
                    "selected_path_count": paths.len(),
                    "staged_path_count_before": staged_paths_before.len(),
                    "index_clean_before": index_clean,
                    "staged_by_tool": staged_by_tool,
                    "index_restored": index_restored
                }),
            });
        }
    }

    let staged_paths = git_staged_paths(&target.root)?;
    let actual_head_at_commit = git_rev_parse(&target.root, "HEAD").unwrap_or_default();
    if !old_head.eq_ignore_ascii_case(&actual_head_at_commit) {
        let index_restored = restore_clean_index_after_git_transaction_failure(
            &target.root,
            staged_by_tool,
            index_clean,
        );
        return Err(git_transaction_head_changed(
            expected_head,
            &old_head,
            &actual_head_at_commit,
            paths.len(),
            staged_paths_before.len(),
            staged_paths.len(),
            index_clean,
            staged_by_tool,
            index_restored,
        ));
    }

    let mut commit_args = vec!["commit".to_string(), "-m".to_string(), message.to_string()];
    if allow_empty {
        commit_args.push("--allow-empty".into());
    }
    let committed = run_git_strings(&target.root, &commit_args, Duration::from_secs(60))?;
    if !committed.success {
        let index_restored = restore_clean_index_after_git_transaction_failure(
            &target.root,
            staged_by_tool,
            index_clean,
        );
        return Err(WorkspaceError::ToolDetails {
            code: "GIT_COMMIT_FAILED",
            message: committed.stderr.trim().to_string(),
            category: "runtime",
            retryable: false,
            details: json!({
                "transaction_stage": "commit",
                "stdout": committed.stdout,
                "expected_head": expected_head,
                "actual_head_before": old_head,
                "actual_head_at_commit": actual_head_at_commit,
                "selected_path_count": paths.len(),
                "staged_path_count_before": staged_paths_before.len(),
                "staged_path_count": staged_paths.len(),
                "index_clean_before": index_clean,
                "staged_by_tool": staged_by_tool,
                "index_restored": index_restored
            }),
        });
    }
    let new_head = git_rev_parse(&target.root, "HEAD").unwrap_or_default();
    let status = git_status(ws, &json!({"path": target.repo_path}))?;
    Ok(tool_ok(json!({
        "dry_run": false,
        "applied": true,
        "commit": new_head,
        "previous_head": old_head,
        "expected_head": expected_head,
        "actual_head_before": actual_head_before,
        "actual_head_at_commit": actual_head_at_commit,
        "actual_head_after": new_head,
        "transaction_stage": "committed",
        "selected_path_count": paths.len(),
        "staged_path_count_before": staged_paths_before.len(),
        "staged_path_count": staged_paths.len(),
        "index_clean_before": index_clean,
        "staged_by_tool": staged_by_tool,
        "index_restored": false,
        "message": message,
        "paths": paths,
        "all": all,
        "stdout": committed.stdout.trim(),
        "repo": target.metadata(),
        "status": status,
        "affected_files": paths.iter().map(|path| json!({"path": path, "operation": "commit"})).collect::<Vec<_>>(),
        "warnings": []
    })))
}

pub fn git_push(ws: &Workspace, args: &Value) -> Result<Value, WorkspaceError> {
    let target = resolve_git_target(ws, args)?;
    verify_expected_head(&target, args)?;
    let remote = validate_git_ref(
        args.get("remote")
            .and_then(Value::as_str)
            .unwrap_or("origin"),
    )?;
    let branch = match args.get("branch").and_then(Value::as_str) {
        Some(branch) => validate_git_ref(branch)?,
        None if target.branch != "HEAD" => target.branch.as_str(),
        None => {
            return Err(WorkspaceError::invalid_argument(
                "branch is required when HEAD is detached",
            ))
        }
    };
    let dry_run = args
        .get("dry_run")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let set_upstream = args
        .get("set_upstream")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let mut git_args = vec!["push".to_string()];
    if dry_run {
        git_args.push("--dry-run".into());
    }
    if set_upstream {
        git_args.push("--set-upstream".into());
    }
    git_args.push(remote.to_string());
    git_args.push(branch.to_string());

    let completed = run_git_strings(&target.root, &git_args, Duration::from_secs(120))?;
    if !completed.success {
        let lowered = completed.stderr.to_ascii_lowercase();
        if lowered.contains("authentication failed")
            || lowered.contains("failed to authenticate")
            || lowered.contains("could not read username")
        {
            return Err(WorkspaceError::ToolDetails {
                code: "GIT_AUTHENTICATION_FAILED",
                message: completed.stderr.trim().to_string(),
                category: "authentication",
                retryable: true,
                details: json!({
                    "remote": remote,
                    "branch": branch,
                    "suggestion": "Refresh the Git credential/token for this remote, then retry the same git_push request."
                }),
            });
        }
        return Err(WorkspaceError::ToolDetails {
            code: "GIT_PUSH_FAILED",
            message: completed.stderr.trim().to_string(),
            category: "git",
            retryable: false,
            details: json!({
                "remote": remote,
                "branch": branch,
                "stdout": completed.stdout
            }),
        });
    }

    let status = git_status(ws, &json!({"path": target.repo_path}))?;
    Ok(tool_ok(json!({
        "dry_run": dry_run,
        "applied": !dry_run,
        "remote": remote,
        "branch": branch,
        "stdout": completed.stdout.trim(),
        "stderr": completed.stderr.trim(),
        "repo": target.metadata(),
        "status": status,
        "affected_files": [],
        "warnings": []
    })))
}

pub fn git_restore(ws: &Workspace, args: &Value) -> Result<Value, WorkspaceError> {
    prevalidate_git_paths(ws, args)?;
    let target = resolve_git_target(ws, args)?;
    verify_expected_head(&target, args)?;
    if !args
        .get("confirm")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(WorkspaceError::ToolDetails {
            code: "DANGEROUS_OPERATION_REQUIRES_CONFIRMATION",
            message: "git_restore discards or unstages changes and requires confirm=true".into(),
            category: "permission",
            retryable: false,
            details: json!({}),
        });
    }
    let paths = git_paths(ws, &target, args)?;
    if paths.is_empty() {
        return Err(WorkspaceError::invalid_argument("paths is required"));
    }
    let staged = args.get("staged").and_then(Value::as_bool).unwrap_or(false);
    let worktree = args
        .get("worktree")
        .and_then(Value::as_bool)
        .unwrap_or(!staged);
    if !staged && !worktree {
        return Err(WorkspaceError::invalid_argument(
            "staged or worktree must be true",
        ));
    }
    let mut git_args = vec!["restore".to_string()];
    if staged {
        git_args.push("--staged".into());
    }
    if worktree {
        git_args.push("--worktree".into());
    }
    if let Some(source) = args.get("source").and_then(Value::as_str) {
        git_args.push(format!("--source={}", validate_git_ref(source)?));
    }
    git_args.push("--".into());
    git_args.extend(paths.clone());
    if args
        .get("dry_run")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Ok(tool_ok(json!({
            "dry_run": true,
            "applied": false,
            "paths": paths,
            "staged": staged,
            "worktree": worktree,
            "repo": target.metadata(),
            "warnings": []
        })));
    }
    let completed = run_git_strings(&target.root, &git_args, Duration::from_secs(20))?;
    if !completed.success {
        return Err(git_error(&completed.stderr));
    }
    let status = git_status(ws, &json!({"path": target.repo_path}))?;
    Ok(tool_ok(json!({
        "dry_run": false,
        "applied": true,
        "paths": paths,
        "staged": staged,
        "worktree": worktree,
        "repo": target.metadata(),
        "status": status,
        "affected_files": paths.iter().map(|path| json!({"path": path, "operation": "restore"})).collect::<Vec<_>>(),
        "warnings": []
    })))
}

fn resolve_git_target(ws: &Workspace, args: &Value) -> Result<GitTarget, WorkspaceError> {
    let repo_path = args.get("repo_path").and_then(Value::as_str).unwrap_or(".");
    let resolved = ws.resolve_existing(repo_path)?;
    if !resolved.path.is_dir() {
        return Err(WorkspaceError::not_a_directory(
            "repo_path must be a directory",
        ));
    }
    let metadata = run_git(
        &resolved.path,
        &[
            "rev-parse",
            "--show-toplevel",
            "--absolute-git-dir",
            "--git-common-dir",
            "HEAD",
        ],
        Duration::from_secs(5),
    )?;
    if !metadata.success {
        let message = metadata.stderr.trim();
        return Err(WorkspaceError::Tool {
            code: "NOT_GIT_REPOSITORY",
            message: if message.is_empty() {
                "repo_path is not inside a Git repository".into()
            } else {
                message.to_string()
            },
            category: "validation",
            retryable: false,
        });
    }
    let values = metadata
        .stdout
        .lines()
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if values.len() < 4 {
        return Err(WorkspaceError::Tool {
            code: "GIT_REPOSITORY_METADATA_INCOMPLETE",
            message: "Git repository metadata response was incomplete".into(),
            category: "runtime",
            retryable: true,
        });
    }
    let root = PathBuf::from(values[0]);
    let git_dir = values[1].to_string();
    let common_dir = if values[2].is_empty() {
        git_dir.clone()
    } else {
        values[2].to_string()
    };
    let head = if values[3].is_empty() {
        "missing".into()
    } else {
        values[3].to_string()
    };
    let branch_result = run_git(
        &resolved.path,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        Duration::from_secs(5),
    )?;
    let branch = if branch_result.success && !branch_result.stdout.trim().is_empty() {
        branch_result.stdout.trim().to_string()
    } else {
        "HEAD".into()
    };
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(
            [
                root.to_string_lossy().as_ref(),
                git_dir.as_str(),
                common_dir.as_str(),
                branch.as_str(),
                head.as_str(),
            ]
            .join("\0")
            .as_bytes(),
        )
    );
    let canonical_repo_path = relative_display(ws.root(), &root);
    let target = GitTarget {
        repo_path: if canonical_repo_path.is_empty() {
            ".".to_string()
        } else {
            canonical_repo_path
        },
        root,
        git_dir,
        common_dir,
        branch,
        head,
        fingerprint,
    };
    if let Some(expected) = args
        .get("expected_repo_fingerprint")
        .and_then(Value::as_str)
    {
        if !expected.eq_ignore_ascii_case(&target.fingerprint) {
            return Err(WorkspaceError::ToolDetails {
                code: "GIT_REPO_TARGET_MISMATCH",
                message: "Git repository/worktree target changed since preflight".into(),
                category: "conflict",
                retryable: true,
                details: json!({
                    "expected_repo_fingerprint": expected,
                    "actual_repo_fingerprint": target.fingerprint,
                    "repo": target.metadata(),
                    "suggestion": "Call git_status with path=repo_path and retry with the returned repo_fingerprint"
                }),
            });
        }
    }
    Ok(target)
}

fn verify_expected_head(target: &GitTarget, args: &Value) -> Result<(), WorkspaceError> {
    if let Some(expected) = args.get("expected_head").and_then(Value::as_str) {
        let actual = &target.head;
        if !expected.eq_ignore_ascii_case(&actual) {
            return Err(WorkspaceError::ToolDetails {
                code: "GIT_HEAD_MISMATCH",
                message: "Git HEAD changed since preflight".into(),
                category: "conflict",
                retryable: true,
                details: json!({
                    "expected_head": expected,
                    "actual_head": actual,
                    "repo": target.metadata()
                }),
            });
        }
    }
    Ok(())
}

fn validate_branch_name(root: &Path, name: &str) -> Result<(), WorkspaceError> {
    let completed = run_git(
        root,
        &["check-ref-format", "--branch", name],
        Duration::from_secs(5),
    )?;
    if completed.success {
        Ok(())
    } else {
        Err(WorkspaceError::invalid_argument(format!(
            "Invalid branch name: {name}"
        )))
    }
}

fn canonical_repo_relative_path(target: &GitTarget, value: &str) -> String {
    let normalized = value.replace('\\', "/");
    let normalized = normalized.strip_prefix("./").unwrap_or(&normalized);
    if target.repo_path != "." {
        let prefix = format!("{}/", target.repo_path.trim_end_matches(['/', '\\']));
        if let Some(relative) = normalized.strip_prefix(&prefix) {
            return relative.to_string();
        }
    }
    normalized.to_string()
}

fn git_read_paths(
    ws: &Workspace,
    target: &GitTarget,
    args: &Value,
) -> Result<Vec<String>, WorkspaceError> {
    let mut paths = Vec::new();
    if let Some(path) = args.get("path").and_then(Value::as_str) {
        ws.reject_unsafe_text(path)?;
        let canonical = canonical_repo_relative_path(target, path);
        ws.reject_unsafe_text(&canonical)?;
        paths.push(canonical);
    }
    if let Some(items) = args.get("paths").and_then(Value::as_array) {
        for item in items {
            let path = item
                .as_str()
                .ok_or_else(|| WorkspaceError::invalid_argument("paths entries must be strings"))?;
            ws.reject_unsafe_text(path)?;
            let canonical = canonical_repo_relative_path(target, path);
            ws.reject_unsafe_text(&canonical)?;
            paths.push(canonical);
        }
    }
    Ok(paths)
}

fn git_paths(
    ws: &Workspace,
    target: &GitTarget,
    args: &Value,
) -> Result<Vec<String>, WorkspaceError> {
    let mut paths = Vec::new();
    if let Some(items) = args.get("paths").and_then(Value::as_array) {
        for item in items {
            let path = item
                .as_str()
                .ok_or_else(|| WorkspaceError::invalid_argument("paths entries must be strings"))?;
            ws.reject_unsafe_text(path)?;
            let canonical = canonical_repo_relative_path(target, path);
            ws.reject_unsafe_text(&canonical)?;
            let workspace_path = if target.repo_path == "." {
                canonical.clone()
            } else {
                format!(
                    "{}/{}",
                    target.repo_path.trim_end_matches(['/', '\\']),
                    canonical
                )
            };
            ws.reject_protected_write_path(&workspace_path)?;
            paths.push(canonical);
        }
    }
    Ok(paths)
}

fn prevalidate_git_paths(ws: &Workspace, args: &Value) -> Result<(), WorkspaceError> {
    if let Some(items) = args.get("paths").and_then(Value::as_array) {
        for item in items {
            let path = item
                .as_str()
                .ok_or_else(|| WorkspaceError::invalid_argument("paths entries must be strings"))?;
            ws.reject_unsafe_text(path)?;
            ws.reject_protected_write_path(path)?;
        }
    }
    Ok(())
}

fn git_index_clean(root: &Path) -> Result<bool, WorkspaceError> {
    let completed = run_git(
        root,
        &["diff", "--cached", "--quiet", "--exit-code"],
        Duration::from_secs(10),
    )?;
    match completed.exit_code {
        0 => Ok(true),
        1 => Ok(false),
        _ => Err(git_error(&completed.stderr)),
    }
}

fn git_staged_paths(root: &Path) -> Result<Vec<String>, WorkspaceError> {
    let completed = run_git(
        root,
        &["diff", "--cached", "--name-only", "-z", "--"],
        Duration::from_secs(10),
    )?;
    if !completed.success {
        return Err(git_error(&completed.stderr));
    }
    Ok(completed
        .stdout
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect())
}

fn restore_clean_index_after_git_transaction_failure(
    root: &Path,
    staged_by_tool: bool,
    index_clean_before: bool,
) -> bool {
    if !staged_by_tool || !index_clean_before {
        return false;
    }
    run_git(
        root,
        &["reset", "--quiet", "HEAD", "--"],
        Duration::from_secs(10),
    )
    .is_ok_and(|output| output.success)
}

#[allow(clippy::too_many_arguments)]
fn git_transaction_head_changed(
    expected_head: Option<&str>,
    actual_head_before: &str,
    actual_head_at_commit: &str,
    selected_path_count: usize,
    staged_path_count_before: usize,
    staged_path_count: usize,
    index_clean_before: bool,
    staged_by_tool: bool,
    index_restored: bool,
) -> WorkspaceError {
    WorkspaceError::ToolDetails {
        code: "GIT_HEAD_CHANGED_DURING_TRANSACTION",
        message: "Git HEAD changed after transaction preflight and before commit".into(),
        category: "conflict",
        retryable: true,
        details: json!({
            "transaction_stage": "pre_commit_head_check",
            "expected_head": expected_head,
            "actual_head_before": actual_head_before,
            "actual_head_at_commit": actual_head_at_commit,
            "selected_path_count": selected_path_count,
            "staged_path_count_before": staged_path_count_before,
            "staged_path_count": staged_path_count,
            "index_clean_before": index_clean_before,
            "staged_by_tool": staged_by_tool,
            "index_restored": index_restored,
            "suggestion": "Refresh git_status/git_diff and retry git_commit with the new expected_head."
        }),
    }
}

fn run_git_strings(
    cwd: &std::path::Path,
    args: &[String],
    limit: Duration,
) -> Result<GitOutput, WorkspaceError> {
    let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    run_git(cwd, &refs, limit)
}

fn validate_git_ref(ref_name: &str) -> Result<&str, WorkspaceError> {
    if ref_name.is_empty()
        || ref_name.starts_with('-')
        || ref_name.contains('\0')
        || ref_name.contains('\n')
        || ref_name.contains('\r')
    {
        return Err(WorkspaceError::invalid_argument("Invalid git revision."));
    }
    Ok(ref_name)
}

fn parse_git_blame_porcelain(output: &str) -> Vec<Value> {
    let commit_re = Regex::new(r"^[0-9a-fA-F^]{40}").expect("valid regex");
    let mut rows = Vec::new();
    let mut current: serde_json::Map<String, Value> = serde_json::Map::new();

    for raw in output.lines() {
        let parts: Vec<&str> = raw.split_whitespace().collect();
        if parts.len() >= 3 && commit_re.is_match(parts[0]) {
            current = serde_json::Map::new();
            current.insert("commit".into(), json!(parts[0].trim_start_matches('^')));
            if parts[1].chars().all(|c| c.is_ascii_digit()) {
                current.insert("original_line".into(), json!(parts[1].parse::<i64>().ok()));
            }
            if parts[2].chars().all(|c| c.is_ascii_digit()) {
                current.insert("line".into(), json!(parts[2].parse::<i64>().ok()));
            }
            continue;
        }
        if let Some(author) = raw.strip_prefix("author ") {
            current.insert("author".into(), json!(author));
            continue;
        }
        if let Some(mail) = raw.strip_prefix("author-mail ") {
            current.insert(
                "author_mail".into(),
                json!(mail.trim_matches(|c| c == '<' || c == '>')),
            );
            continue;
        }
        if let Some(time) = raw.strip_prefix("author-time ") {
            let value = if time.chars().all(|c| c.is_ascii_digit()) {
                json!(time.parse::<i64>().ok())
            } else {
                json!(time)
            };
            current.insert("author_time".into(), value);
            continue;
        }
        if let Some(summary) = raw.strip_prefix("summary ") {
            current.insert("summary".into(), json!(summary));
            continue;
        }
        if let Some(content) = raw.strip_prefix('\t') {
            let mut row = current.clone();
            row.insert("content".into(), json!(content));
            rows.push(Value::Object(row));
        }
    }
    rows
}

struct GitOutput {
    success: bool,
    exit_code: i32,
    stdout: String,
    stderr: String,
}

fn read_child_pipe<R>(mut reader: R) -> thread::JoinHandle<io::Result<Vec<u8>>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        Ok(bytes)
    })
}

fn join_child_pipe(reader: Option<thread::JoinHandle<io::Result<Vec<u8>>>>) -> io::Result<Vec<u8>> {
    match reader {
        Some(reader) => reader
            .join()
            .map_err(|_| io::Error::other("Git output reader panicked"))?,
        None => Ok(Vec::new()),
    }
}

fn wait_for_child_output(mut child: Child, limit: Duration) -> io::Result<(Output, bool)> {
    #[cfg(windows)]
    let _process_tree_guard = crate::platform::attach_process_tree(child.id());
    let stdout = child.stdout.take().map(read_child_pipe);
    let stderr = child.stderr.take().map(read_child_pipe);
    let started = Instant::now();
    let (status, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (status, false),
            Ok(None) if started.elapsed() < limit => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                #[cfg(windows)]
                {
                    let _ = crate::platform::platform().terminate_process_tree(child.id());
                }
                let _ = child.kill();
                break (child.wait()?, true);
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        }
    };
    Ok((
        Output {
            status,
            stdout: join_child_pipe(stdout)?,
            stderr: join_child_pipe(stderr)?,
        },
        timed_out,
    ))
}

fn run_git(
    cwd: &std::path::Path,
    args: &[&str],
    limit: Duration,
) -> Result<GitOutput, WorkspaceError> {
    let command_args = args
        .iter()
        .map(|arg| (*arg).to_string())
        .collect::<Vec<_>>();
    let git_env = [("GIT_TERMINAL_PROMPT".to_string(), "0".to_string())];
    let mut cmd = std_command_for_workspace_with_env("git", &command_args, cwd, &git_env, &[]);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let child = cmd
        .spawn()
        .map_err(|e| git_error(&format!("git not available: {e}")))?;
    let (output, timed_out) = wait_for_child_output(child, limit)
        .map_err(|error| git_error(&format!("git process failed: {error}")))?;
    if timed_out {
        return Err(WorkspaceError::ToolDetails {
            code: "GIT_COMMAND_TIMEOUT",
            message: format!("Git command exceeded {} ms", limit.as_millis()),
            category: "timeout",
            retryable: true,
            details: json!({
                "timeout_ms": limit.as_millis(),
                "stderr": String::from_utf8_lossy(&output.stderr).trim()
            }),
        });
    }
    Ok(GitOutput {
        success: output.status.success(),
        exit_code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn run_git_diff(
    root: &std::path::Path,
    context: u64,
    path_filters: &[String],
    cached: bool,
) -> Result<String, WorkspaceError> {
    let unified = format!("--unified={context}");
    let mut args = vec!["diff", unified.as_str()];
    if cached {
        args.push("--cached");
    }
    if !path_filters.is_empty() {
        args.push("--");
        for p in path_filters {
            args.push(p.as_str());
        }
    }
    let completed = run_git(root, &args, Duration::from_secs(10))?;
    if completed.exit_code != 0 && completed.exit_code != 1 {
        return Err(git_error(&completed.stderr));
    }
    Ok(completed.stdout)
}

fn is_git_repo(root: &std::path::Path) -> bool {
    run_git(root, &["rev-parse", "--git-dir"], Duration::from_secs(5))
        .map(|o| o.success)
        .unwrap_or(false)
}

fn git_rev_parse(cwd: &std::path::Path, rev: &str) -> Option<String> {
    run_git(cwd, &["rev-parse", rev], Duration::from_secs(5))
        .ok()
        .filter(|o| o.success)
        .map(|o| o.stdout.trim().to_string())
}

fn git_value(cwd: &Path, args: &[&str]) -> Option<String> {
    run_git(cwd, args, Duration::from_secs(5))
        .ok()
        .filter(|output| output.success)
        .map(|output| output.stdout.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_branch_line(line: &str) -> (String, String, i64, i64) {
    let (branch_part, tracking) = line
        .split_once("...")
        .map(|(b, t)| (b.to_string(), t.to_string()))
        .unwrap_or((line.to_string(), String::new()));
    let branch = branch_part
        .split_once(' ')
        .map(|(b, _)| b.to_string())
        .unwrap_or(branch_part);
    let mut ahead = 0i64;
    let mut behind = 0i64;
    let mut upstream = tracking.clone();
    if let Some(idx) = tracking.find(' ') {
        upstream = tracking[..idx].to_string();
        let meta = &tracking[idx + 1..];
        for token in meta.split(',') {
            let token = token.trim();
            if let Some(n) = token.strip_prefix("ahead ") {
                ahead = n.trim().parse().unwrap_or(0);
            } else if let Some(n) = token.strip_prefix("behind ") {
                behind = n.trim().parse().unwrap_or(0);
            }
        }
    }
    (branch, upstream, ahead, behind)
}

fn parse_diff_files(diff: &str) -> Vec<Value> {
    let mut files = Vec::new();
    for line in diff.lines() {
        if let Some(path) = line.strip_prefix("+++ b/") {
            files.push(json!({
                "path": path,
                "status": "modified",
                "binary": false
            }));
        } else if line.starts_with("--- /dev/null") {
            continue;
        } else if let Some(path) = line.strip_prefix("--- a/") {
            if !files.iter().any(|f| f["path"] == path) {
                files.push(json!({
                    "path": path,
                    "status": "modified",
                    "binary": false
                }));
            }
        }
    }
    files
}

fn git_error(message: &str) -> WorkspaceError {
    WorkspaceError::Tool {
        code: "GIT_ERROR",
        message: message.to_string(),
        category: "runtime",
        retryable: false,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        canonical_repo_relative_path, git_diff, git_show, git_transaction_head_changed,
        git_worktree, wait_for_child_output, GitTarget,
    };
    use crate::tools::workspace::{Workspace, WorkspaceError};
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    #[test]
    fn git_pathspecs_accept_repo_or_workspace_relative_inputs_and_return_repo_relative_paths() {
        let target = GitTarget {
            repo_path: "nested-repo".into(),
            root: PathBuf::from("nested-repo"),
            git_dir: "git-dir".into(),
            common_dir: "git-dir".into(),
            branch: "main".into(),
            head: "head".into(),
            fingerprint: "fingerprint".into(),
        };
        assert_eq!(
            canonical_repo_relative_path(&target, "tracked.txt"),
            "tracked.txt"
        );
        assert_eq!(
            canonical_repo_relative_path(&target, "nested-repo/tracked.txt"),
            "tracked.txt"
        );
        assert_eq!(
            canonical_repo_relative_path(&target, "nested-repo\\tracked.txt"),
            "tracked.txt"
        );
    }

    #[test]
    fn git_diff_resolves_nested_repo_and_canonicalizes_workspace_relative_pathspecs() {
        let workspace = tempfile::tempdir().expect("workspace");
        let nested = workspace.path().join("nested-repo");
        fs::create_dir(&nested).expect("nested repo");
        let run = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(&nested)
                .output()
                .expect("run git");
            assert!(
                output.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(&["init"]);
        fs::write(nested.join("tracked.txt"), "before\n").expect("write fixture");
        run(&["add", "tracked.txt"]);
        run(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "initial",
        ]);
        fs::write(nested.join("tracked.txt"), "after\n").expect("update fixture");
        let ws = Workspace::new(workspace.path().to_path_buf()).expect("workspace model");

        let repo_relative = git_diff(
            &ws,
            &json!({"repo_path": "nested-repo", "paths": ["tracked.txt"]}),
        )
        .expect("repo relative diff");
        assert_eq!(repo_relative["ok"], true, "{repo_relative}");
        assert_eq!(repo_relative["repo"]["repo_path"], "nested-repo");
        assert_eq!(repo_relative["paths"], json!(["tracked.txt"]));
        assert!(repo_relative["diff"]
            .as_str()
            .unwrap_or_default()
            .contains("-before"));
        assert!(repo_relative["diff"]
            .as_str()
            .unwrap_or_default()
            .contains("+after"));

        let workspace_relative = git_diff(
            &ws,
            &json!({"repo_path": "nested-repo", "paths": ["nested-repo/tracked.txt"]}),
        )
        .expect("workspace relative diff");
        assert_eq!(workspace_relative["paths"], json!(["tracked.txt"]));
        assert_eq!(workspace_relative["diff"], repo_relative["diff"]);

        let shown = git_show(
            &ws,
            &json!({"repo_path": "nested-repo", "rev": "HEAD", "include_diff": false}),
        )
        .expect("nested repo show");
        assert_eq!(shown["ok"], true, "{shown}");
        assert_eq!(shown["repo"]["repo_path"], "nested-repo");
        assert!(shown["content"]
            .as_str()
            .unwrap_or_default()
            .contains("initial"));
    }

    #[test]
    fn transaction_head_change_returns_retryable_conflict_metadata() {
        let error = git_transaction_head_changed(
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            2,
            0,
            2,
            true,
            true,
            true,
        );
        match error {
            WorkspaceError::ToolDetails {
                code,
                category,
                retryable,
                details,
                ..
            } => {
                assert_eq!(code, "GIT_HEAD_CHANGED_DURING_TRANSACTION");
                assert_eq!(category, "conflict");
                assert!(retryable);
                assert_eq!(details["transaction_stage"], "pre_commit_head_check");
                assert_eq!(details["selected_path_count"], 2);
                assert_eq!(details["staged_path_count"], 2);
                assert_eq!(details["index_clean_before"], true);
                assert_eq!(details["index_restored"], true);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn git_worktree_manages_linked_worktree_lifecycle() {
        let root = tempfile::tempdir().expect("temp repo");
        let run = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(root.path())
                .output()
                .expect("run git");
            assert!(
                output.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(&["init"]);
        fs::write(root.path().join("README.md"), "fixture\n").expect("write fixture");
        run(&["add", "README.md"]);
        run(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "initial",
        ]);

        let ws = Workspace::new(root.path().to_path_buf()).expect("workspace");
        let created = git_worktree(
            &ws,
            &json!({
                "action": "create",
                "path": ".worktrees/managed",
                "branch": "feature/managed-worktree",
                "start_point": "HEAD"
            }),
        )
        .expect("create linked worktree");
        assert_eq!(created["ok"], true);
        assert_eq!(created["applied"], true);
        assert!(root.path().join(".worktrees/managed").is_dir());

        let denied = git_worktree(
            &ws,
            &json!({
                "action": "remove",
                "path": ".worktrees/managed",
                "delete_branch": true
            }),
        )
        .expect_err("remove must require confirmation");
        match denied {
            WorkspaceError::ToolDetails { code, .. } => {
                assert_eq!(code, "DANGEROUS_OPERATION_REQUIRES_CONFIRMATION");
            }
            other => panic!("unexpected worktree removal error: {other:?}"),
        }

        let removed = git_worktree(
            &ws,
            &json!({
                "action": "remove",
                "path": ".worktrees/managed",
                "delete_branch": true,
                "confirm": true
            }),
        )
        .expect("remove linked worktree");
        assert_eq!(removed["ok"], true);
        assert_eq!(removed["applied"], true);
        assert_eq!(removed["branch_deleted"], true);
        assert!(!root.path().join(".worktrees/managed").exists());
    }

    #[test]
    fn git_worktree_supports_existing_branch_defaults_dry_run_and_metadata() {
        let root = tempfile::tempdir().expect("temp repo");
        let run = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(root.path())
                .output()
                .expect("run git");
            assert!(
                output.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(&["init"]);
        fs::write(root.path().join("README.md"), "fixture\n").expect("write fixture");
        run(&["add", "README.md"]);
        run(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "initial",
        ]);
        run(&["branch", "feature/existing-worktree"]);

        let ws = Workspace::new(root.path().to_path_buf()).expect("workspace");
        let planned = git_worktree(
            &ws,
            &json!({
                "action": "create", "branch": "feature/existing-worktree", "dry_run": true
            }),
        )
        .expect("plan existing branch worktree");
        assert_eq!(planned["branch_mode"], "existing");
        assert_eq!(planned["path"], ".worktrees/feature-existing-worktree");

        let created = git_worktree(
            &ws,
            &json!({
                "action": "create", "branch": "feature/existing-worktree"
            }),
        )
        .expect("create existing branch worktree");
        assert_eq!(created["branch_mode"], "existing");
        let worktree_path = ".worktrees/feature-existing-worktree";
        assert!(root.path().join(worktree_path).is_dir());

        let listed = git_worktree(&ws, &json!({"action": "list"})).expect("list worktrees");
        let entries = listed["worktrees"].as_array().expect("worktree array");
        let primary = entries
            .iter()
            .find(|entry| entry["path"] == ".")
            .expect("primary entry");
        let managed = entries
            .iter()
            .find(|entry| entry["path"] == worktree_path)
            .expect("managed entry");
        assert_eq!(primary["primary"], true);
        assert_eq!(primary["inside_workspace"], true);
        assert_eq!(primary["removable"], false);
        assert_eq!(managed["primary"], false);
        assert_eq!(managed["inside_workspace"], true);
        assert_eq!(managed["managed"], true);
        assert_eq!(managed["removable"], true);
        assert!(managed["dirty"].is_boolean());
        assert!(managed["branch_merged"].is_boolean());

        let dry_remove = git_worktree(
            &ws,
            &json!({
                "action": "remove", "path": worktree_path, "dry_run": true
            }),
        )
        .expect("dry-run remove without confirmation");
        assert_eq!(dry_remove["applied"], false);
        assert!(root.path().join(worktree_path).exists());

        let removed = git_worktree(
            &ws,
            &json!({
                "action": "remove", "path": worktree_path, "confirm": true
            }),
        )
        .expect("remove existing branch worktree");
        assert_eq!(removed["applied"], true);
        assert!(!root.path().join(worktree_path).exists());
    }

    #[test]
    fn child_output_timeout_is_enforced() {
        #[cfg(windows)]
        let mut command = {
            let mut command = Command::new("cmd");
            command.args(["/d", "/c", "ping 127.0.0.1 -n 3 >nul"]);
            command
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut command = Command::new("sh");
            command.args(["-c", "sleep 1"]);
            command
        };
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let child = command.spawn().expect("spawn timeout test child");
        let started = Instant::now();
        let (_, timed_out) = wait_for_child_output(child, Duration::from_millis(50))
            .expect("collect timed out child");
        assert!(timed_out);
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
