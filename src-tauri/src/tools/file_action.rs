use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use glob::Pattern;
use ignore::WalkBuilder;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::tools::context::ToolContext;
use crate::tools::workspace::{tool_ok, Workspace, WorkspaceError};

mod transaction;

use transaction::{
    apply_guarded, bounded_text, changed_paths, format_json_file, prepare_mirror, read_originals,
    snapshot_tree, truncate_text, unified_diff, MirrorGuard,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActionKind {
    Format,
    Lint,
    Fix,
    OrganizeImports,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActionMode {
    Plan,
    Check,
    Apply,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActionScope {
    Files,
    Changed,
    Staged,
    Project,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionRequest {
    pub kind: ActionKind,
    pub mode: ActionMode,
    pub scope: ActionScope,
    pub paths: Vec<String>,
    pub formatter: Option<String>,
    pub strict: bool,
    pub include_patterns: Vec<String>,
    pub exclude_patterns: Vec<String>,
    pub max_files: usize,
    pub timeout_ms: u64,
    pub expected_sha256: BTreeMap<String, String>,
    pub confirm: bool,
    pub max_diff_bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedFile {
    pub path: String,
    pub adapter_id: String,
    pub config_path: Option<String>,
    pub selection_source: String,
    pub rust_edition: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkippedFile {
    pub path: String,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionGroup {
    pub adapter_id: String,
    pub config_path: Option<String>,
    pub files: Vec<String>,
    pub mutation_risk: String,
    pub custom: bool,
    pub rust_edition: Option<String>,
    command_template: Option<CustomCommandTemplate>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionPlan {
    pub files_requested: usize,
    pub files_supported: usize,
    pub groups: Vec<ActionGroup>,
    pub files: Vec<PlannedFile>,
    pub skipped: Vec<SkippedFile>,
}

#[derive(Clone, Copy)]
struct AdapterSpec {
    id: &'static str,
    extensions: &'static [&'static str],
    config_names: &'static [&'static str],
    mutation_risk: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CustomCommandTemplate {
    program: String,
    args: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CustomAdapter {
    id: String,
    program: String,
    extensions: Vec<String>,
    args: Vec<String>,
    config_path: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SelectedAdapter {
    id: String,
    config_path: Option<PathBuf>,
    selection_source: &'static str,
    mutation_risk: String,
    custom: bool,
    command_template: Option<CustomCommandTemplate>,
}

const ADAPTERS: &[AdapterSpec] = &[
    AdapterSpec {
        id: "rustfmt",
        extensions: &["rs"],
        config_names: &["rustfmt.toml", ".rustfmt.toml", "Cargo.toml"],
        mutation_risk: "targeted",
    },
    AdapterSpec {
        id: "biome",
        extensions: &[
            "js", "jsx", "mjs", "cjs", "ts", "tsx", "json", "jsonc", "css",
        ],
        config_names: &["biome.json", "biome.jsonc", "package.json"],
        mutation_risk: "targeted",
    },
    AdapterSpec {
        id: "dprint",
        extensions: &[
            "js", "jsx", "mjs", "cjs", "ts", "tsx", "json", "jsonc", "md", "toml",
        ],
        config_names: &["dprint.json", ".dprint.json", "package.json"],
        mutation_risk: "targeted",
    },
    AdapterSpec {
        id: "prettier",
        extensions: &[
            "js", "jsx", "mjs", "cjs", "ts", "tsx", "json", "jsonc", "yaml", "yml", "md",
            "markdown", "css", "scss", "less", "html", "vue", "svelte",
        ],
        config_names: &[
            ".prettierrc",
            ".prettierrc.json",
            ".prettierrc.yaml",
            ".prettierrc.yml",
            ".prettierrc.js",
            ".prettierrc.cjs",
            "prettier.config.js",
            "package.json",
            "prettier.config.cjs",
        ],
        mutation_risk: "targeted",
    },
    AdapterSpec {
        id: "ruff",
        extensions: &["py", "pyi"],
        config_names: &["ruff.toml", ".ruff.toml", "pyproject.toml"],
        mutation_risk: "targeted",
    },
    AdapterSpec {
        id: "black",
        extensions: &["py", "pyi"],
        config_names: &["pyproject.toml"],
        mutation_risk: "targeted",
    },
    AdapterSpec {
        id: "gofmt",
        extensions: &["go"],
        config_names: &[],
        mutation_risk: "targeted",
    },
    AdapterSpec {
        id: "clang-format",
        extensions: &["c", "h", "cc", "cpp", "cxx", "hpp", "java", "proto"],
        config_names: &[".clang-format", "_clang-format"],
        mutation_risk: "targeted",
    },
    AdapterSpec {
        id: "csharpier",
        extensions: &["cs"],
        config_names: &[".csharpierrc", ".csharpierrc.json"],
        mutation_risk: "targeted",
    },
    AdapterSpec {
        id: "ktfmt",
        extensions: &["kt", "kts"],
        config_names: &[],
        mutation_risk: "targeted",
    },
    AdapterSpec {
        id: "ktlint",
        extensions: &["kt", "kts"],
        config_names: &[".editorconfig"],
        mutation_risk: "targeted",
    },
    AdapterSpec {
        id: "shfmt",
        extensions: &["sh", "bash", "zsh"],
        config_names: &[".editorconfig"],
        mutation_risk: "targeted",
    },
    AdapterSpec {
        id: "terraform-fmt",
        extensions: &["tf", "tfvars", "hcl"],
        config_names: &[],
        mutation_risk: "targeted",
    },
    AdapterSpec {
        id: "taplo",
        extensions: &["toml"],
        config_names: &["taplo.toml", ".taplo.toml"],
        mutation_risk: "targeted",
    },
    AdapterSpec {
        id: "builtin-json",
        extensions: &["json"],
        config_names: &[],
        mutation_risk: "targeted",
    },
];

const GENERATED_MANIFESTS: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "poetry.lock",
    "Pipfile.lock",
    "composer.lock",
    "go.sum",
];

impl ActionRequest {
    pub fn from_format_args(args: &Value) -> Result<Self, WorkspaceError> {
        let mode = match args.get("mode").and_then(Value::as_str).unwrap_or("plan") {
            "plan" => ActionMode::Plan,
            "check" => ActionMode::Check,
            "apply" => ActionMode::Apply,
            other => {
                return Err(WorkspaceError::invalid_argument(format!(
                    "Unsupported format mode: {other}"
                )))
            }
        };
        let scope = match args.get("scope").and_then(Value::as_str).unwrap_or("files") {
            "files" => ActionScope::Files,
            "changed" => ActionScope::Changed,
            "staged" => ActionScope::Staged,
            "project" => ActionScope::Project,
            other => {
                return Err(WorkspaceError::invalid_argument(format!(
                    "Unsupported format scope: {other}"
                )))
            }
        };
        let paths = string_array(args, "paths")?;
        if scope == ActionScope::Files && paths.is_empty() {
            return Err(WorkspaceError::invalid_argument(
                "format_files with scope=files requires at least one path",
            ));
        }
        let formatter = args
            .get("formatter")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && *value != "auto")
            .map(str::to_string);
        if let Some(formatter) = formatter.as_deref() {
            if !valid_adapter_id(formatter) {
                return Err(WorkspaceError::invalid_argument(format!(
                    "Invalid formatter adapter ID: {formatter}"
                )));
            }
        }
        Ok(Self {
            kind: ActionKind::Format,
            mode,
            scope,
            paths,
            formatter,
            strict: args.get("strict").and_then(Value::as_bool).unwrap_or(false),
            include_patterns: string_array(args, "include_patterns")?,
            exclude_patterns: string_array(args, "exclude_patterns")?,
            max_files: args
                .get("max_files")
                .and_then(Value::as_u64)
                .unwrap_or(500)
                .clamp(1, 10_000) as usize,
            timeout_ms: args
                .get("timeout_ms")
                .and_then(Value::as_u64)
                .unwrap_or(120_000)
                .clamp(1, 600_000),
            expected_sha256: string_map(args, "expected_sha256")?,
            confirm: args
                .get("confirm")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            max_diff_bytes: args
                .get("max_diff_bytes")
                .and_then(Value::as_u64)
                .unwrap_or(262_144)
                .clamp(1_024, 1_048_576) as usize,
        })
    }
}

pub fn plan_actions(ws: &Workspace, request: &ActionRequest) -> Result<ActionPlan, WorkspaceError> {
    let custom_adapters = load_custom_adapters(ws.root())?;
    let paths = collect_requested_files(ws, request)?;
    let files_requested = paths.len();
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    let mut groups: BTreeMap<(String, Option<String>, Option<String>), ActionGroup> =
        BTreeMap::new();

    for path in paths {
        let display = relative_path(ws.root(), &path);
        let file_name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if GENERATED_MANIFESTS.contains(&file_name) {
            skipped.push(SkippedFile {
                path: display,
                reason: "generated_manifest".into(),
            });
            continue;
        }
        if is_binary_file(&path)? {
            skipped.push(SkippedFile {
                path: display,
                reason: "binary_file".into(),
            });
            continue;
        }
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let Some(selected) =
            select_adapter(ws.root(), &path, &extension, request, &custom_adapters)?
        else {
            skipped.push(SkippedFile {
                path: display,
                reason: "unsupported_file_type".into(),
            });
            continue;
        };
        let config_path = selected
            .config_path
            .as_ref()
            .map(|value| relative_path(ws.root(), value));
        let rust_edition =
            (selected.id == "rustfmt").then(|| rust_edition_for_file(ws.root(), &path));
        let planned = PlannedFile {
            path: display.clone(),
            adapter_id: selected.id.clone(),
            config_path: config_path.clone(),
            selection_source: selected.selection_source.into(),
            rust_edition: rust_edition.clone(),
        };
        let group = groups
            .entry((
                selected.id.clone(),
                config_path.clone(),
                rust_edition.clone(),
            ))
            .or_insert_with(|| ActionGroup {
                adapter_id: selected.id.clone(),
                config_path,
                files: Vec::new(),
                mutation_risk: selected.mutation_risk.clone(),
                custom: selected.custom,
                rust_edition,
                command_template: selected.command_template.clone(),
            });
        group.files.push(display);
        files.push(planned);
    }

    if request.strict && !skipped.is_empty() {
        return Err(WorkspaceError::ToolDetails {
            code: "FORMAT_UNSUPPORTED_FILES",
            message: "One or more requested files cannot be formatted in strict mode.".into(),
            category: "validation",
            retryable: false,
            details: json!({
                "skipped": skipped.iter().map(|item| json!({"path": item.path, "reason": item.reason})).collect::<Vec<_>>()
            }),
        });
    }

    files.sort_by(|left, right| left.path.cmp(&right.path));
    skipped.sort_by(|left, right| left.path.cmp(&right.path));
    let mut groups = groups.into_values().collect::<Vec<_>>();
    for group in &mut groups {
        group.files.sort();
    }
    let files_supported = files.len();
    Ok(ActionPlan {
        files_requested,
        files_supported,
        groups,
        files,
        skipped,
    })
}

fn string_array(args: &Value, key: &str) -> Result<Vec<String>, WorkspaceError> {
    let Some(value) = args.get(key) else {
        return Ok(Vec::new());
    };
    let values = value.as_array().ok_or_else(|| {
        WorkspaceError::invalid_argument(format!("{key} must be an array of strings"))
    })?;
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .ok_or_else(|| {
                    WorkspaceError::invalid_argument(format!(
                        "{key} entries must be non-empty strings"
                    ))
                })
        })
        .collect()
}

fn string_map(args: &Value, key: &str) -> Result<BTreeMap<String, String>, WorkspaceError> {
    let Some(value) = args.get(key) else {
        return Ok(BTreeMap::new());
    };
    let object = value.as_object().ok_or_else(|| {
        WorkspaceError::invalid_argument(format!("{key} must be an object of string values"))
    })?;
    object
        .iter()
        .map(|(path, value)| {
            let value = value.as_str().ok_or_else(|| {
                WorkspaceError::invalid_argument(format!("{key}.{path} must be a string"))
            })?;
            if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(WorkspaceError::invalid_argument(format!(
                    "{key}.{path} must be a 64-character SHA-256"
                )));
            }
            Ok((path.clone(), value.to_ascii_lowercase()))
        })
        .collect()
}

fn collect_requested_files(
    ws: &Workspace,
    request: &ActionRequest,
) -> Result<Vec<PathBuf>, WorkspaceError> {
    if matches!(request.scope, ActionScope::Changed | ActionScope::Staged) {
        return collect_git_scope_files(ws, request);
    }
    let roots = if request.scope == ActionScope::Project && request.paths.is_empty() {
        vec![".".to_string()]
    } else {
        request.paths.clone()
    };
    let includes = compile_patterns(&request.include_patterns, "include_patterns")?;
    let excludes = compile_patterns(&request.exclude_patterns, "exclude_patterns")?;
    let mut collected = BTreeSet::new();
    for raw in roots {
        let resolved = ws.resolve_existing(&raw)?;
        if resolved.path.is_file() {
            let display = relative_path(ws.root(), &resolved.path);
            if matches_patterns(&display, &includes, &excludes) {
                collected.insert(resolved.path);
            }
            continue;
        }
        if !resolved.path.is_dir() {
            continue;
        }
        let mut builder = WalkBuilder::new(&resolved.path);
        builder
            .follow_links(false)
            .hidden(false)
            .require_git(false)
            .filter_entry(|entry| {
                entry.file_name().to_str().map_or(true, |name| {
                    !name.eq_ignore_ascii_case(".git")
                        && !crate::tools::workspace::DEFAULT_EXCLUDED_NAMES.contains(&name)
                })
            });
        for entry in builder.build().filter_map(Result::ok) {
            if collected.len() >= request.max_files {
                break;
            }
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            let path = entry.path();
            if !ws.is_safe_read_path(path) || ws.is_ignored_path(path, false, false, false) {
                continue;
            }
            let display = relative_path(ws.root(), path);
            if matches_patterns(&display, &includes, &excludes) {
                collected.insert(path.to_path_buf());
            }
        }
    }
    Ok(collected.into_iter().take(request.max_files).collect())
}

fn collect_git_scope_files(
    ws: &Workspace,
    request: &ActionRequest,
) -> Result<Vec<PathBuf>, WorkspaceError> {
    let status = crate::tools::git::git_status(
        ws,
        &json!({"path": ".", "include_untracked": true, "max_entries": request.max_files.saturating_mul(4)}),
    )?;
    if status.get("is_repo").and_then(Value::as_bool) != Some(true) {
        return Err(WorkspaceError::ToolDetails {
            code: "FORMAT_GIT_SCOPE_UNAVAILABLE",
            message: format!(
                "format_files scope={} requires a Git repository",
                scope_name(request.scope)
            ),
            category: "validation",
            retryable: false,
            details: json!({"scope": scope_name(request.scope)}),
        });
    }
    let includes = compile_patterns(&request.include_patterns, "include_patterns")?;
    let excludes = compile_patterns(&request.exclude_patterns, "exclude_patterns")?;
    let roots = request
        .paths
        .iter()
        .map(|path| path.trim_end_matches('/').replace('\\', "/"))
        .collect::<Vec<_>>();
    let mut collected = BTreeSet::new();
    for entry in status
        .get("entries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(path) = entry.get("path").and_then(Value::as_str) else {
            continue;
        };
        let index_status = entry
            .get("index_status")
            .and_then(Value::as_str)
            .unwrap_or(" ");
        if request.scope == ActionScope::Staged && matches!(index_status, " " | "?") {
            continue;
        }
        let normalized = path.replace('\\', "/");
        if !roots.is_empty()
            && !roots.iter().any(|root| {
                normalized == *root
                    || normalized
                        .strip_prefix(root)
                        .is_some_and(|suffix| suffix.starts_with('/'))
            })
        {
            continue;
        }
        if !matches_patterns(&normalized, &includes, &excludes) {
            continue;
        }
        let candidate = ws.root().join(path);
        if candidate.is_file() && ws.is_safe_read_path(&candidate) {
            collected.insert(candidate);
        }
        if collected.len() >= request.max_files {
            break;
        }
    }
    Ok(collected.into_iter().collect())
}

fn scope_name(scope: ActionScope) -> &'static str {
    match scope {
        ActionScope::Files => "files",
        ActionScope::Changed => "changed",
        ActionScope::Staged => "staged",
        ActionScope::Project => "project",
    }
}

fn compile_patterns(values: &[String], key: &str) -> Result<Vec<Pattern>, WorkspaceError> {
    values
        .iter()
        .map(|value| {
            Pattern::new(value).map_err(|error| {
                WorkspaceError::invalid_argument(format!("Invalid {key} glob {value}: {error}"))
            })
        })
        .collect()
}

fn matches_patterns(path: &str, includes: &[Pattern], excludes: &[Pattern]) -> bool {
    (includes.is_empty() || includes.iter().any(|pattern| pattern.matches(path)))
        && !excludes.iter().any(|pattern| pattern.matches(path))
}

fn valid_adapter_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn load_custom_adapters(
    workspace_root: &Path,
) -> Result<BTreeMap<String, CustomAdapter>, WorkspaceError> {
    let config_path = workspace_root.join(".coding-tools/formatters.json");
    if !config_path.is_file() {
        return Ok(BTreeMap::new());
    }
    let bytes = fs::read(&config_path).map_err(|error| {
        formatter_config_error(
            "Could not read custom formatter configuration",
            json!({"path": ".coding-tools/formatters.json", "error": error.to_string()}),
        )
    })?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
        formatter_config_error(
            "Custom formatter configuration is not valid JSON",
            json!({"path": ".coding-tools/formatters.json", "error": error.to_string()}),
        )
    })?;
    let formatters = value
        .get("formatters")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            formatter_config_error(
                "Custom formatter configuration requires a formatters object",
                json!({"path": ".coding-tools/formatters.json"}),
            )
        })?;
    if formatters.len() > 50 {
        return Err(formatter_config_error(
            "Custom formatter configuration exceeds 50 adapters",
            json!({"adapter_count": formatters.len()}),
        ));
    }

    let mut adapters = BTreeMap::new();
    for (id, raw) in formatters {
        if !valid_adapter_id(id) {
            return Err(formatter_config_error(
                "Custom formatter ID contains unsupported characters",
                json!({"adapter_id": id}),
            ));
        }
        if adapter(id).is_some() {
            return Err(formatter_config_error(
                "Custom formatter ID conflicts with a built-in adapter",
                json!({"adapter_id": id}),
            ));
        }
        let object = raw.as_object().ok_or_else(|| {
            formatter_config_error(
                "Custom formatter entry must be an object",
                json!({"adapter_id": id}),
            )
        })?;
        let program = object
            .get("program")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                formatter_config_error(
                    "Custom formatter requires a program",
                    json!({"adapter_id": id}),
                )
            })?;
        validate_custom_relative_path(id, "program", program)?;

        let extension_values = object
            .get("extensions")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                formatter_config_error(
                    "Custom formatter requires an extensions array",
                    json!({"adapter_id": id}),
                )
            })?;
        if extension_values.is_empty() || extension_values.len() > 50 {
            return Err(formatter_config_error(
                "Custom formatter extensions must contain 1 to 50 entries",
                json!({"adapter_id": id}),
            ));
        }
        let mut extensions = Vec::new();
        for extension in extension_values {
            let extension = extension
                .as_str()
                .map(|value| value.trim_start_matches('.').to_ascii_lowercase())
                .filter(|value| {
                    !value.is_empty()
                        && value.bytes().all(|byte| {
                            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'+' | b'-')
                        })
                })
                .ok_or_else(|| {
                    formatter_config_error(
                        "Custom formatter extension is invalid",
                        json!({"adapter_id": id}),
                    )
                })?;
            if !extensions.contains(&extension) {
                extensions.push(extension);
            }
        }

        let arg_values = object
            .get("args")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                formatter_config_error(
                    "Custom formatter requires an args array",
                    json!({"adapter_id": id}),
                )
            })?;
        if arg_values.len() > 100 {
            return Err(formatter_config_error(
                "Custom formatter args exceed 100 entries",
                json!({"adapter_id": id}),
            ));
        }
        let mut args = Vec::new();
        let mut has_file_placeholder = false;
        for raw_arg in arg_values {
            let arg = raw_arg.as_str().ok_or_else(|| {
                formatter_config_error(
                    "Custom formatter args must be strings",
                    json!({"adapter_id": id}),
                )
            })?;
            if matches!(arg, "{files}" | "{file}") {
                has_file_placeholder = true;
            } else if !matches!(arg, "{config}" | "{workspace}")
                && (arg.contains('{') || arg.contains('}'))
            {
                return Err(formatter_config_error(
                    "Custom formatter contains an unsupported placeholder",
                    json!({"adapter_id": id, "argument": arg}),
                ));
            }
            args.push(arg.to_string());
        }
        if !has_file_placeholder {
            return Err(formatter_config_error(
                "Custom formatter args require {files} or {file}",
                json!({"adapter_id": id}),
            ));
        }

        let config_path = object
            .get("config")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(path) = config_path.as_deref() {
            validate_custom_relative_path(id, "config", path)?;
        }
        adapters.insert(
            id.clone(),
            CustomAdapter {
                id: id.clone(),
                program: program.to_string(),
                extensions,
                args,
                config_path,
            },
        );
    }
    Ok(adapters)
}

fn validate_custom_relative_path(
    adapter_id: &str,
    field: &str,
    value: &str,
) -> Result<(), WorkspaceError> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| component == std::path::Component::ParentDir)
    {
        return Err(formatter_config_error(
            "Custom formatter paths must stay inside the workspace",
            json!({"adapter_id": adapter_id, "field": field, "value": value}),
        ));
    }
    Ok(())
}

fn formatter_config_error(message: &str, details: Value) -> WorkspaceError {
    WorkspaceError::ToolDetails {
        code: "FORMATTER_CONFIG_INVALID",
        message: message.into(),
        category: "validation",
        retryable: false,
        details,
    }
}

fn select_adapter(
    workspace_root: &Path,
    file: &Path,
    extension: &str,
    request: &ActionRequest,
    custom_adapters: &BTreeMap<String, CustomAdapter>,
) -> Result<Option<SelectedAdapter>, WorkspaceError> {
    if let Some(explicit) = request.formatter.as_deref() {
        if let Some(custom) = custom_adapters.get(explicit) {
            if !custom.extensions.iter().any(|value| value == extension) {
                if request.strict {
                    return Err(WorkspaceError::invalid_argument(format!(
                        "Formatter {explicit} does not support {}",
                        relative_path(workspace_root, file)
                    )));
                }
                return Ok(None);
            }
            return Ok(Some(selected_custom_adapter(workspace_root, custom)));
        }
        let Some(spec) = adapter(explicit) else {
            return Err(WorkspaceError::invalid_argument(format!(
                "Unknown formatter adapter: {explicit}"
            )));
        };
        if !spec.extensions.contains(&extension) {
            if request.strict {
                return Err(WorkspaceError::invalid_argument(format!(
                    "Formatter {explicit} does not support {}",
                    relative_path(workspace_root, file)
                )));
            }
            return Ok(None);
        }
        let config = nearest_supported_config(workspace_root, file, spec);
        return Ok(Some(selected_static_adapter(spec, config, "explicit")));
    }

    let custom_matches = custom_adapters
        .values()
        .filter(|custom| custom.extensions.iter().any(|value| value == extension))
        .collect::<Vec<_>>();
    if custom_matches.len() > 1 {
        return Err(WorkspaceError::ToolDetails {
            code: "FORMATTER_AMBIGUOUS",
            message: format!("Multiple custom formatters support .{extension}"),
            category: "validation",
            retryable: false,
            details: json!({
                "extension": extension,
                "adapter_ids": custom_matches.iter().map(|adapter| adapter.id.as_str()).collect::<Vec<_>>(),
                "suggestion": "Specify formatter explicitly"
            }),
        });
    }
    if let Some(custom) = custom_matches.first() {
        return Ok(Some(selected_custom_adapter(workspace_root, custom)));
    }

    let mut configured = ADAPTERS
        .iter()
        .filter(|spec| spec.extensions.contains(&extension) && !spec.config_names.is_empty())
        .filter_map(|spec| {
            nearest_supported_config(workspace_root, file, spec).map(|config| {
                let depth = config.components().count();
                (spec, config, depth)
            })
        })
        .collect::<Vec<_>>();
    configured.sort_by(|left, right| right.2.cmp(&left.2));
    if let Some((spec, config, _)) = configured.into_iter().next() {
        let source = if matches!(
            config.file_name().and_then(|value| value.to_str()),
            Some("package.json" | "pyproject.toml" | "Cargo.toml" | "go.mod")
        ) {
            "manifest"
        } else {
            "nearest_config"
        };
        return Ok(Some(selected_static_adapter(spec, Some(config), source)));
    }

    let Some(default_id) = default_adapter_for_extension(extension) else {
        return Ok(None);
    };
    Ok(adapter(default_id).map(|spec| selected_static_adapter(spec, None, "language_default")))
}

fn selected_static_adapter(
    spec: &AdapterSpec,
    config_path: Option<PathBuf>,
    selection_source: &'static str,
) -> SelectedAdapter {
    SelectedAdapter {
        id: spec.id.into(),
        config_path,
        selection_source,
        mutation_risk: spec.mutation_risk.into(),
        custom: false,
        command_template: None,
    }
}

fn selected_custom_adapter(workspace_root: &Path, custom: &CustomAdapter) -> SelectedAdapter {
    SelectedAdapter {
        id: custom.id.clone(),
        config_path: custom
            .config_path
            .as_ref()
            .map(|path| workspace_root.join(path)),
        selection_source: "workspace_config",
        mutation_risk: "custom".into(),
        custom: true,
        command_template: Some(CustomCommandTemplate {
            program: custom.program.clone(),
            args: custom.args.clone(),
        }),
    }
}

fn nearest_supported_config(
    workspace_root: &Path,
    file: &Path,
    spec: &AdapterSpec,
) -> Option<PathBuf> {
    let mut current = file.parent()?;
    loop {
        for name in spec.config_names {
            let candidate = current.join(name);
            if candidate.is_file() && config_supports_adapter(spec.id, &candidate) {
                return Some(candidate);
            }
        }
        if current == workspace_root {
            break;
        }
        current = current.parent()?;
        if !current.starts_with(workspace_root) {
            break;
        }
    }
    None
}

fn config_supports_adapter(adapter_id: &str, path: &Path) -> bool {
    match path.file_name().and_then(|value| value.to_str()) {
        Some("pyproject.toml") => {
            let text = fs::read_to_string(path)
                .unwrap_or_default()
                .to_ascii_lowercase();
            match adapter_id {
                "ruff" => text.contains("[tool.ruff"),
                "black" => text.contains("[tool.black"),
                _ => true,
            }
        }
        Some("package.json") => {
            let Ok(value) = fs::read_to_string(path)
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .ok_or(())
            else {
                return false;
            };
            let mut signals = String::new();
            for key in [
                "dependencies",
                "devDependencies",
                "optionalDependencies",
                "peerDependencies",
                "scripts",
            ] {
                if let Some(object) = value.get(key).and_then(Value::as_object) {
                    for (name, value) in object {
                        signals.push_str(&name.to_ascii_lowercase());
                        signals.push(' ');
                        if let Some(value) = value.as_str() {
                            signals.push_str(&value.to_ascii_lowercase());
                            signals.push(' ');
                        }
                    }
                }
            }
            match adapter_id {
                "biome" => signals.contains("@biomejs/biome") || signals.contains(" biome"),
                "dprint" => signals.contains("dprint"),
                "prettier" => signals.contains("prettier"),
                _ => true,
            }
        }
        _ => true,
    }
}

fn cargo_manifest_rust_edition(
    path: &Path,
) -> Option<(bool, Option<String>, bool, Option<String>)> {
    let content = fs::read_to_string(path).ok()?;
    let value = toml::from_str::<toml::Value>(&content).ok()?;
    let package = value.get("package").and_then(toml::Value::as_table);
    let package_present = package.is_some();
    let package_edition = package
        .and_then(|table| table.get("edition"))
        .and_then(toml::Value::as_str)
        .map(str::to_string);
    let package_inherits_workspace = package
        .and_then(|table| table.get("edition"))
        .and_then(toml::Value::as_table)
        .and_then(|edition| edition.get("workspace"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(false);
    let workspace_edition = value
        .get("workspace")
        .and_then(toml::Value::as_table)
        .and_then(|workspace| workspace.get("package"))
        .and_then(toml::Value::as_table)
        .and_then(|package| package.get("edition"))
        .and_then(toml::Value::as_str)
        .map(str::to_string);
    Some((
        package_present,
        package_edition,
        package_inherits_workspace,
        workspace_edition,
    ))
}

fn rust_edition_for_file(workspace_root: &Path, file: &Path) -> String {
    let Some(mut current) = file.parent() else {
        return "2021".into();
    };
    let mut inherits_workspace = false;
    loop {
        let manifest = current.join("Cargo.toml");
        if manifest.is_file() {
            if let Some((package_present, package_edition, package_inherits, workspace_edition)) =
                cargo_manifest_rust_edition(&manifest)
            {
                if inherits_workspace {
                    if let Some(edition) = workspace_edition {
                        return edition;
                    }
                } else if package_present {
                    if let Some(edition) = package_edition {
                        return edition;
                    }
                    if package_inherits {
                        inherits_workspace = true;
                        if let Some(edition) = workspace_edition {
                            return edition;
                        }
                    } else {
                        return "2015".into();
                    }
                }
            }
        }
        if current == workspace_root {
            break;
        }
        let Some(parent) = current.parent() else {
            break;
        };
        if !parent.starts_with(workspace_root) {
            break;
        }
        current = parent;
    }
    "2021".into()
}

fn adapter(id: &str) -> Option<&'static AdapterSpec> {
    ADAPTERS.iter().find(|spec| spec.id == id)
}

fn default_adapter_for_extension(extension: &str) -> Option<&'static str> {
    match extension {
        "rs" => Some("rustfmt"),
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "yaml" | "yml" | "md" | "markdown"
        | "css" | "scss" | "less" | "html" | "vue" | "svelte" => Some("prettier"),
        "json" => Some("builtin-json"),
        "jsonc" => Some("prettier"),
        "py" | "pyi" => Some("ruff"),
        "go" => Some("gofmt"),
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "java" | "proto" => Some("clang-format"),
        "cs" => Some("csharpier"),
        "kt" | "kts" => Some("ktfmt"),
        "sh" | "bash" | "zsh" => Some("shfmt"),
        "tf" | "tfvars" | "hcl" => Some("terraform-fmt"),
        "toml" => Some("taplo"),
        _ => None,
    }
}

fn is_binary_file(path: &Path) -> Result<bool, WorkspaceError> {
    let bytes = fs::read(path)
        .map_err(|_| WorkspaceError::not_found(format!("File not found: {}", path.display())))?;
    Ok(bytes.iter().take(8192).any(|byte| *byte == 0))
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdapterCommand {
    pub executable_candidates: Vec<String>,
    pub args: Vec<String>,
}

fn build_adapter_command(request: &RunnerRequest) -> Result<AdapterCommand, WorkspaceError> {
    if let Some(command) = request.command_override.as_ref() {
        return Ok(command.clone());
    }
    if request.adapter_id == "rustfmt" {
        let edition = request.rust_edition.as_deref().unwrap_or("2021");
        let mut args = vec!["--edition".to_string(), edition.to_string()];
        args.extend(request.files.iter().cloned());
        return Ok(AdapterCommand {
            executable_candidates: vec!["rustfmt".into()],
            args,
        });
    }
    let (candidates, prefix): (&[&str], &[&str]) = match request.adapter_id.as_str() {
        "prettier" => (&["prettier"], &["--write"]),
        "biome" => (&["biome"], &["format", "--write"]),
        "dprint" => (&["dprint"], &["fmt"]),
        "ruff" => (&["ruff"], &["format"]),
        "black" => (&["black"], &[]),
        "gofmt" => (&["gofmt"], &["-w"]),
        "clang-format" => (&["clang-format"], &["-i"]),
        "csharpier" => (&["csharpier", "dotnet-csharpier"], &["format"]),
        "ktfmt" => (&["ktfmt"], &[]),
        "ktlint" => (&["ktlint"], &["-F"]),
        "shfmt" => (&["shfmt"], &["-w"]),
        "terraform-fmt" => (&["terraform"], &["fmt"]),
        "taplo" => (&["taplo"], &["format"]),
        other => {
            return Err(WorkspaceError::invalid_argument(format!(
                "No command adapter is registered for {other}"
            )))
        }
    };
    let mut args = prefix
        .iter()
        .map(|value| (*value).to_string())
        .collect::<Vec<_>>();
    args.extend(request.files.iter().cloned());
    Ok(AdapterCommand {
        executable_candidates: candidates
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        args,
    })
}

fn render_custom_command(
    template: &CustomCommandTemplate,
    files: &[String],
    config_path: Option<&str>,
) -> Result<AdapterCommand, WorkspaceError> {
    let mut args = Vec::new();
    for argument in &template.args {
        match argument.as_str() {
            "{files}" | "{file}" => args.extend(files.iter().cloned()),
            "{workspace}" => args.push(".".into()),
            "{config}" => {
                let config_path = config_path.ok_or_else(|| {
                    formatter_config_error(
                        "Custom formatter uses {config} without a configured path",
                        json!({"program": template.program}),
                    )
                })?;
                args.push(config_path.to_string());
            }
            value => args.push(value.to_string()),
        }
    }
    Ok(AdapterCommand {
        executable_candidates: vec![template.program.clone()],
        args,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunnerRequest {
    pub adapter_id: String,
    pub mirror_root: PathBuf,
    pub files: Vec<String>,
    pub config_path: Option<String>,
    pub rust_edition: Option<String>,
    pub timeout_ms: u64,
    pub command_override: Option<AdapterCommand>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunnerOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub trait ActionRunner {
    fn run(&self, request: &RunnerRequest) -> Result<RunnerOutput, WorkspaceError>;
}

#[derive(Clone, Debug)]
pub struct SystemRunner {
    workspace_root: PathBuf,
}

impl SystemRunner {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self { workspace_root }
    }
}

impl ActionRunner for SystemRunner {
    fn run(&self, request: &RunnerRequest) -> Result<RunnerOutput, WorkspaceError> {
        let command = build_adapter_command(request)?;
        let Some(executable) =
            resolve_formatter_executable(&self.workspace_root, &command.executable_candidates)
        else {
            return Err(WorkspaceError::ToolDetails {
                code: "FORMATTER_UNAVAILABLE",
                message: format!("Formatter {} is not installed", request.adapter_id),
                category: "runtime",
                retryable: true,
                details: json!({
                    "adapter_id": request.adapter_id,
                    "executable_candidates": command.executable_candidates,
                    "suggestion": "Install the project-local formatter or select another formatter adapter"
                }),
            });
        };

        let output_dir = std::env::temp_dir()
            .join("coding-tools-format-output")
            .join(Uuid::new_v4().to_string());
        fs::create_dir_all(&output_dir).map_err(|error| WorkspaceError::ToolDetails {
            code: "FORMATTER_START_FAILED",
            message: format!("Could not create formatter output directory: {error}"),
            category: "runtime",
            retryable: true,
            details: json!({"adapter_id": request.adapter_id}),
        })?;
        let output_guard = TempDirectoryGuard(output_dir.clone());
        let stdout_path = output_dir.join("stdout.log");
        let stderr_path = output_dir.join("stderr.log");
        let stdout_file =
            File::create(&stdout_path).map_err(|error| WorkspaceError::ToolDetails {
                code: "FORMATTER_START_FAILED",
                message: format!("Could not prepare formatter stdout: {error}"),
                category: "runtime",
                retryable: true,
                details: json!({"adapter_id": request.adapter_id}),
            })?;
        let stderr_file =
            File::create(&stderr_path).map_err(|error| WorkspaceError::ToolDetails {
                code: "FORMATTER_START_FAILED",
                message: format!("Could not prepare formatter stderr: {error}"),
                category: "runtime",
                retryable: true,
                details: json!({"adapter_id": request.adapter_id}),
            })?;

        let wsl_workspace = crate::workspace::parse_wsl_path(&request.mirror_root).is_some();
        let executable_text = executable.to_string_lossy().into_owned();
        let mut process = crate::platform::wsl::std_command_for_workspace_clean_env(
            &executable_text,
            &command.args,
            &request.mirror_root,
        );
        process
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout_file))
            .stderr(Stdio::from(stderr_file));
        if !wsl_workspace {
            copy_safe_formatter_environment(&mut process);
        }
        let mut child = process
            .spawn()
            .map_err(|error| WorkspaceError::ToolDetails {
                code: "FORMATTER_START_FAILED",
                message: format!("Could not start formatter {}: {error}", request.adapter_id),
                category: "runtime",
                retryable: true,
                details: json!({
                    "adapter_id": request.adapter_id,
                    "executable": executable,
                    "args": command.args
                }),
            })?;

        let started = Instant::now();
        let status = loop {
            if let Some(status) = child
                .try_wait()
                .map_err(|error| WorkspaceError::ToolDetails {
                    code: "FORMATTER_WAIT_FAILED",
                    message: format!(
                        "Could not wait for formatter {}: {error}",
                        request.adapter_id
                    ),
                    category: "runtime",
                    retryable: true,
                    details: json!({"adapter_id": request.adapter_id}),
                })?
            {
                break status;
            }
            if started.elapsed() >= Duration::from_millis(request.timeout_ms) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(WorkspaceError::ToolDetails {
                    code: "FORMATTER_TIMEOUT",
                    message: format!(
                        "Formatter {} exceeded {} ms",
                        request.adapter_id, request.timeout_ms
                    ),
                    category: "timeout",
                    retryable: true,
                    details: json!({
                        "adapter_id": request.adapter_id,
                        "timeout_ms": request.timeout_ms
                    }),
                });
            }
            thread::sleep(Duration::from_millis(20));
        };

        let stdout = read_bounded_output(&stdout_path, 65_536);
        let stderr = read_bounded_output(&stderr_path, 65_536);
        drop(output_guard);
        Ok(RunnerOutput {
            exit_code: status.code().unwrap_or(-1),
            stdout,
            stderr,
        })
    }
}

struct TempDirectoryGuard(PathBuf);

impl Drop for TempDirectoryGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn copy_safe_formatter_environment(command: &mut Command) {
    for key in [
        "PATH",
        "PATHEXT",
        "SYSTEMROOT",
        "WINDIR",
        "COMSPEC",
        "HOME",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "TEMP",
        "TMP",
        "TMPDIR",
        "CARGO_HOME",
        "RUSTUP_HOME",
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "LANG",
        "LC_ALL",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
}

fn read_bounded_output(path: &Path, max_bytes: usize) -> String {
    match fs::read(path) {
        Ok(bytes) => {
            let end = bytes.len().min(max_bytes);
            String::from_utf8_lossy(&bytes[..end]).into_owned()
        }
        Err(_) => String::new(),
    }
}

fn resolve_formatter_executable(workspace_root: &Path, candidates: &[String]) -> Option<PathBuf> {
    let wsl_workspace = crate::workspace::parse_wsl_path(workspace_root).is_some();
    for candidate in candidates {
        if candidate.contains(['/', '\\']) {
            let path = workspace_root.join(candidate);
            if let Some(resolved) = canonical_workspace_file(workspace_root, &path) {
                return Some(resolved);
            }
            continue;
        }
        for relative in workspace_executable_candidates(candidate, wsl_workspace) {
            let path = workspace_root.join(relative);
            if let Some(resolved) = canonical_workspace_file(workspace_root, &path) {
                return Some(resolved);
            }
        }
    }
    if wsl_workspace {
        return candidates
            .iter()
            .find(|candidate| !candidate.contains(['/', '\\']))
            .map(PathBuf::from);
    }
    candidates
        .iter()
        .filter(|candidate| !candidate.contains(['/', '\\']))
        .find_map(|candidate| which::which(candidate).ok())
}

fn canonical_workspace_file(workspace_root: &Path, candidate: &Path) -> Option<PathBuf> {
    let canonical_root = workspace_root.canonicalize().ok()?;
    let resolved = candidate.canonicalize().ok()?;
    (resolved.starts_with(canonical_root) && resolved.is_file()).then_some(resolved)
}

fn workspace_executable_candidates(candidate: &str, wsl_workspace: bool) -> Vec<PathBuf> {
    let mut names = vec![candidate.to_string()];
    if cfg!(windows) && !wsl_workspace {
        names.insert(0, format!("{candidate}.cmd"));
        names.push(format!("{candidate}.exe"));
    }
    let roots = if cfg!(windows) && !wsl_workspace {
        vec![
            "node_modules/.bin",
            ".venv/Scripts",
            "venv/Scripts",
            "bin",
            "tools",
        ]
    } else {
        vec!["node_modules/.bin", ".venv/bin", "venv/bin", "bin", "tools"]
    };
    roots
        .into_iter()
        .flat_map(|root| names.iter().map(move |name| PathBuf::from(root).join(name)))
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionOutcome {
    pub mode: ActionMode,
    pub plan: ActionPlan,
    pub files_changed: Vec<String>,
    pub files_unchanged: Vec<String>,
    pub files_skipped: Vec<SkippedFile>,
    pub unavailable_adapters: Vec<String>,
    pub unexpected_changes: Vec<String>,
    pub diff: String,
    pub diff_truncated: bool,
    pub applied: bool,
}

pub fn execute_actions_with_runner(
    ws: &Workspace,
    request: &ActionRequest,
    runner: &dyn ActionRunner,
) -> Result<ActionOutcome, WorkspaceError> {
    let plan = plan_actions(ws, request)?;
    if request.mode == ActionMode::Plan {
        return Ok(ActionOutcome {
            mode: request.mode,
            files_skipped: plan.skipped.clone(),
            plan,
            files_changed: Vec::new(),
            files_unchanged: Vec::new(),
            unavailable_adapters: Vec::new(),
            unexpected_changes: Vec::new(),
            diff: String::new(),
            diff_truncated: false,
            applied: false,
        });
    }
    if plan.groups.iter().any(|group| group.custom) && !request.confirm {
        return Err(WorkspaceError::ToolDetails {
            code: "CUSTOM_FORMATTER_REQUIRES_CONFIRMATION",
            message: "Custom formatter execution requires confirm=true".into(),
            category: "permission",
            retryable: false,
            details: json!({
                "custom_adapters": plan.groups.iter().filter(|group| group.custom).map(|group| group.adapter_id.as_str()).collect::<Vec<_>>(),
                "suggestion": "Review mode=plan output, then retry with confirm=true"
            }),
        });
    }

    let originals = read_originals(ws, &plan, request)?;
    let mirror = MirrorGuard::create(ws.root())?;
    prepare_mirror(ws, &mirror.root, &plan)?;
    let mut mirror_snapshot = snapshot_tree(&mirror.root)?;
    let mut unavailable_adapters = Vec::new();
    let mut unavailable_files = BTreeSet::new();
    let mut files_skipped = plan.skipped.clone();

    for group in &plan.groups {
        if group.adapter_id == "builtin-json" {
            for path in &group.files {
                format_json_file(&mirror.root.join(path))?;
            }
        } else {
            let runner_request = RunnerRequest {
                adapter_id: group.adapter_id.clone(),
                mirror_root: mirror.root.clone(),
                files: group.files.clone(),
                config_path: group.config_path.clone(),
                rust_edition: group.rust_edition.clone(),
                timeout_ms: request.timeout_ms,
                command_override: group
                    .command_template
                    .as_ref()
                    .map(|template| {
                        render_custom_command(template, &group.files, group.config_path.as_deref())
                    })
                    .transpose()?,
            };
            let output = match runner.run(&runner_request) {
                Ok(output) => output,
                Err(WorkspaceError::ToolDetails {
                    code: "FORMATTER_UNAVAILABLE",
                    ..
                }) if !request.strict => {
                    unavailable_adapters.push(group.adapter_id.clone());
                    for path in &group.files {
                        unavailable_files.insert(path.clone());
                        files_skipped.push(SkippedFile {
                            path: path.clone(),
                            reason: "formatter_unavailable".into(),
                        });
                    }
                    continue;
                }
                Err(error) => return Err(error),
            };
            if output.exit_code != 0 {
                return Err(WorkspaceError::ToolDetails {
                    code: "FORMATTER_FAILED",
                    message: format!("Formatter {} failed", group.adapter_id),
                    category: "runtime",
                    retryable: true,
                    details: json!({
                        "adapter_id": group.adapter_id,
                        "exit_code": output.exit_code,
                        "stdout": bounded_text(&output.stdout, 16_384),
                        "stderr": bounded_text(&output.stderr, 16_384)
                    }),
                });
            }
        }

        let after = snapshot_tree(&mirror.root)?;
        let allowed = group.files.iter().cloned().collect::<BTreeSet<_>>();
        let unexpected = changed_paths(&mirror_snapshot, &after)
            .into_iter()
            .filter(|path| !allowed.contains(path))
            .collect::<Vec<_>>();
        if !unexpected.is_empty() {
            return Err(WorkspaceError::ToolDetails {
                code: "FORMAT_UNEXPECTED_CHANGES",
                message: format!(
                    "Formatter {} changed files outside the requested group",
                    group.adapter_id
                ),
                category: "conflict",
                retryable: false,
                details: json!({
                    "adapter_id": group.adapter_id,
                    "unexpected_changes": unexpected,
                    "allowed_files": group.files
                }),
            });
        }
        mirror_snapshot = after;
    }

    let mut formatted = BTreeMap::new();
    let mut files_changed = Vec::new();
    let mut files_unchanged = Vec::new();
    let mut diff = String::new();
    for file in &plan.files {
        if unavailable_files.contains(&file.path) {
            continue;
        }
        let bytes =
            fs::read(mirror.root.join(&file.path)).map_err(|_| WorkspaceError::ToolDetails {
                code: "FORMATTER_OUTPUT_MISSING",
                message: format!("Formatter did not preserve output for {}", file.path),
                category: "runtime",
                retryable: false,
                details: json!({"path": file.path, "adapter_id": file.adapter_id}),
            })?;
        let original = originals.get(&file.path).expect("planned original");
        if bytes == original.bytes {
            files_unchanged.push(file.path.clone());
            continue;
        }
        let original_text =
            String::from_utf8(original.bytes.clone()).map_err(|_| WorkspaceError::Tool {
                code: "UNSUPPORTED_ENCODING",
                message: format!("File is not valid UTF-8: {}", file.path),
                category: "validation",
                retryable: false,
            })?;
        let formatted_text =
            String::from_utf8(bytes.clone()).map_err(|_| WorkspaceError::Tool {
                code: "FORMATTER_OUTPUT_ENCODING",
                message: format!("Formatter produced non-UTF-8 output: {}", file.path),
                category: "runtime",
                retryable: false,
            })?;
        diff.push_str(&unified_diff(&file.path, &original_text, &formatted_text));
        formatted.insert(file.path.clone(), bytes);
        files_changed.push(file.path.clone());
    }

    files_changed.sort();
    files_unchanged.sort();
    files_skipped.sort_by(|left, right| left.path.cmp(&right.path));
    unavailable_adapters.sort();
    unavailable_adapters.dedup();
    let (diff, diff_truncated) = truncate_text(diff, request.max_diff_bytes);
    let applied = request.mode == ActionMode::Apply && !formatted.is_empty();
    if applied {
        apply_guarded(ws, &originals, &formatted)?;
    }

    Ok(ActionOutcome {
        mode: request.mode,
        files_skipped,
        plan,
        files_changed,
        files_unchanged,
        unavailable_adapters,
        unexpected_changes: Vec::new(),
        diff,
        diff_truncated,
        applied,
    })
}

pub fn format_files(ctx: &ToolContext, args: &Value) -> Result<Value, WorkspaceError> {
    let request = ActionRequest::from_format_args(args)?;
    let runner = SystemRunner::new(ctx.workspace.root().to_path_buf());
    let outcome = execute_actions_with_runner(&ctx.workspace, &request, &runner)?;
    Ok(tool_ok(format_outcome_json(&outcome, request.scope)))
}

fn format_outcome_json(outcome: &ActionOutcome, scope: ActionScope) -> Value {
    json!({
        "status": match outcome.mode {
            ActionMode::Plan => "planned",
            ActionMode::Check => "checked",
            ActionMode::Apply if outcome.applied => "applied",
            ActionMode::Apply => "unchanged",
        },
        "mode": mode_name(outcome.mode),
        "scope": scope_name(scope),
        "files_requested": outcome.plan.files_requested,
        "files_supported": outcome.plan.files_supported,
        "files_changed": outcome.files_changed,
        "files_changed_count": outcome.files_changed.len(),
        "files_unchanged": outcome.files_unchanged,
        "files_unchanged_count": outcome.files_unchanged.len(),
        "files_skipped": outcome.files_skipped.iter().map(|file| json!({
            "path": file.path,
            "reason": file.reason
        })).collect::<Vec<_>>(),
        "files_skipped_count": outcome.files_skipped.len(),
        "groups": outcome.plan.groups.iter().map(|group| json!({
            "adapter_id": group.adapter_id,
            "config_path": group.config_path,
            "files": group.files,
            "rust_edition": group.rust_edition,
            "mutation_risk": group.mutation_risk,
            "custom": group.custom
        })).collect::<Vec<_>>(),
        "formatter_group_count": outcome.plan.groups.len(),
        "custom_formatter_group_count": outcome.plan.groups.iter().filter(|group| group.custom).count(),
        "selection": outcome.plan.files.iter().map(|file| json!({
            "path": file.path,
            "adapter_id": file.adapter_id,
            "config_path": file.config_path,
            "selection_source": file.selection_source,
            "rust_edition": file.rust_edition
        })).collect::<Vec<_>>(),
        "unavailable_adapters": outcome.unavailable_adapters,
        "unexpected_changes": outcome.unexpected_changes,
        "diff": outcome.diff,
        "diff_bytes": outcome.diff.len(),
        "diff_truncated": outcome.diff_truncated,
        "applied": outcome.applied,
        "warnings": if outcome.unavailable_adapters.is_empty() {
            Vec::<String>::new()
        } else {
            vec![format!(
                "Unavailable formatters were skipped: {}",
                outcome.unavailable_adapters.join(", ")
            )]
        }
    })
}

fn mode_name(mode: ActionMode) -> &'static str {
    match mode {
        ActionMode::Plan => "plan",
        ActionMode::Check => "check",
        ActionMode::Apply => "apply",
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;

    use serde_json::json;
    use tempfile::tempdir;

    use super::{
        build_adapter_command, execute_actions_with_runner, plan_actions,
        resolve_formatter_executable, rust_edition_for_file, workspace_executable_candidates,
        ActionRequest, ActionRunner, RunnerOutput, RunnerRequest,
    };
    use crate::tools::workspace::Workspace;

    #[test]
    fn format_request_parses_safe_defaults() {
        let request = ActionRequest::from_format_args(&json!({
            "paths": ["src/lib.rs"],
            "mode": "plan"
        }))
        .expect("request");

        assert_eq!(request.paths, vec!["src/lib.rs"]);
        assert_eq!(request.max_files, 500);
        assert_eq!(request.timeout_ms, 120_000);
        assert!(!request.strict);
    }

    #[test]
    fn wsl_formatter_candidates_use_linux_virtualenv_layout() {
        let candidates = workspace_executable_candidates("ruff", true);
        assert!(candidates.contains(&std::path::PathBuf::from(".venv/bin/ruff")));
        assert!(candidates.contains(&std::path::PathBuf::from("venv/bin/ruff")));
        assert!(!candidates.contains(&std::path::PathBuf::from(".venv/Scripts/ruff.exe")));
        assert!(!candidates
            .iter()
            .any(|path| path.to_string_lossy().ends_with("ruff.cmd")));
    }

    #[test]
    fn planner_prefers_nearest_biome_config_over_prettier() {
        let temp = tempdir().expect("workspace");
        fs::create_dir_all(temp.path().join("apps/web/src")).expect("directories");
        fs::write(temp.path().join(".prettierrc"), "{}\n").expect("prettier config");
        fs::write(temp.path().join("apps/web/biome.json"), "{}\n").expect("biome config");
        fs::write(temp.path().join("apps/web/src/app.ts"), "const x=1\n").expect("source");
        let ws = Workspace::new(temp.path().to_path_buf()).expect("workspace");
        let request = ActionRequest::from_format_args(&json!({
            "paths": ["apps/web/src/app.ts"],
            "mode": "plan"
        }))
        .expect("request");

        let plan = plan_actions(&ws, &request).expect("plan");
        assert_eq!(plan.files_supported, 1);
        assert_eq!(plan.groups.len(), 1);
        assert_eq!(plan.groups[0].adapter_id, "biome");
        assert_eq!(
            plan.groups[0].config_path.as_deref(),
            Some("apps/web/biome.json")
        );
        assert_eq!(plan.files[0].selection_source, "nearest_config");
    }

    #[test]
    fn planner_derives_cargo_editions_and_keeps_rustfmt_groups_edition_safe() {
        let temp = tempdir().expect("workspace");
        for (name, edition) in [("crate-2021", "2021"), ("crate-2024", "2024")] {
            let crate_root = temp.path().join(name);
            fs::create_dir_all(crate_root.join("src")).expect("crate src");
            fs::write(
                crate_root.join("Cargo.toml"),
                format!(
                    "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"{edition}\"\n"
                ),
            )
            .expect("manifest");
            fs::write(crate_root.join("src/lib.rs"), "pub async fn value() {}\n")
                .expect("rust source");
        }
        let ws = Workspace::new(temp.path().to_path_buf()).expect("workspace");
        let request = ActionRequest::from_format_args(&json!({
            "paths": ["crate-2021/src/lib.rs", "crate-2024/src/lib.rs"],
            "mode": "plan",
            "formatter": "rustfmt"
        }))
        .expect("request");
        let plan = plan_actions(&ws, &request).expect("plan");
        assert_eq!(plan.groups.len(), 2);
        let editions = plan
            .groups
            .iter()
            .filter_map(|group| group.rust_edition.as_deref())
            .collect::<Vec<_>>();
        assert_eq!(editions, vec!["2021", "2024"]);
        for group in &plan.groups {
            let runner_request = RunnerRequest {
                adapter_id: group.adapter_id.clone(),
                mirror_root: temp.path().to_path_buf(),
                files: group.files.clone(),
                config_path: group.config_path.clone(),
                rust_edition: group.rust_edition.clone(),
                timeout_ms: 1_000,
                command_override: None,
            };
            let command = build_adapter_command(&runner_request).expect("rustfmt command");
            assert_eq!(command.args[0], "--edition");
            assert_eq!(
                command.args[1],
                group.rust_edition.as_deref().unwrap_or("2021")
            );
        }
    }

    #[test]
    fn rust_edition_inherits_workspace_package_edition() {
        let temp = tempdir().expect("workspace");
        let member = temp.path().join("member");
        fs::create_dir_all(member.join("src")).expect("member src");
        fs::write(
            temp.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"member\"]\n[workspace.package]\nedition = \"2024\"\n",
        )
        .expect("workspace manifest");
        fs::write(
            member.join("Cargo.toml"),
            "[package]\nname = \"member\"\nversion = \"0.1.0\"\nedition.workspace = true\n",
        )
        .expect("member manifest");
        let source = member.join("src/lib.rs");
        fs::write(&source, "pub async fn inherited() {}\n").expect("source");
        assert_eq!(rust_edition_for_file(temp.path(), &source), "2024");
    }

    #[test]
    fn planner_groups_languages_and_skips_generated_binary_and_unknown_files() {
        let temp = tempdir().expect("workspace");
        fs::create_dir_all(temp.path().join("src")).expect("src");
        fs::write(temp.path().join("src/lib.rs"), "fn main(){}\n").expect("rust");
        fs::write(temp.path().join("src/main.py"), "print( 1 )\n").expect("python");
        fs::write(temp.path().join("settings.json"), "{\"a\":1}\n").expect("json");
        fs::write(temp.path().join("package-lock.json"), "{}\n").expect("lock");
        fs::write(temp.path().join("image.bin"), [0_u8, 1, 2, 3]).expect("binary");
        fs::write(temp.path().join("README.unknown"), "text\n").expect("unknown");
        let ws = Workspace::new(temp.path().to_path_buf()).expect("workspace");
        let request = ActionRequest::from_format_args(&json!({
            "paths": [
                "src/lib.rs",
                "src/main.py",
                "settings.json",
                "package-lock.json",
                "image.bin",
                "README.unknown"
            ],
            "mode": "plan"
        }))
        .expect("request");

        let plan = plan_actions(&ws, &request).expect("plan");
        let adapters = plan
            .groups
            .iter()
            .map(|group| group.adapter_id.as_str())
            .collect::<Vec<_>>();
        assert!(adapters.contains(&"rustfmt"));
        assert!(adapters.contains(&"ruff"));
        assert!(adapters.contains(&"builtin-json"));
        assert_eq!(plan.files_supported, 3);
        assert!(plan
            .skipped
            .iter()
            .any(|file| file.path == "package-lock.json" && file.reason == "generated_manifest"));
        assert!(plan
            .skipped
            .iter()
            .any(|file| file.path == "image.bin" && file.reason == "binary_file"));
        assert!(plan
            .skipped
            .iter()
            .any(|file| file.path == "README.unknown" && file.reason == "unsupported_file_type"));
    }

    #[test]
    fn explicit_formatter_overrides_auto_detection_when_compatible() {
        let temp = tempdir().expect("workspace");
        fs::write(temp.path().join("app.ts"), "const x=1\n").expect("source");
        fs::write(temp.path().join("biome.json"), "{}\n").expect("config");
        let ws = Workspace::new(temp.path().to_path_buf()).expect("workspace");
        let request = ActionRequest::from_format_args(&json!({
            "paths": ["app.ts"],
            "mode": "plan",
            "formatter": "prettier"
        }))
        .expect("request");

        let plan = plan_actions(&ws, &request).expect("plan");
        assert_eq!(plan.groups[0].adapter_id, "prettier");
        assert_eq!(plan.files[0].selection_source, "explicit");
    }

    #[test]
    fn pyproject_content_selects_black_when_ruff_is_not_configured() {
        let temp = tempdir().expect("workspace");
        fs::write(
            temp.path().join("pyproject.toml"),
            "[tool.black]\nline-length = 88\n",
        )
        .expect("pyproject");
        fs::write(temp.path().join("app.py"), "print( 1 )\n").expect("python");
        let ws = Workspace::new(temp.path().to_path_buf()).expect("workspace");
        let request = ActionRequest::from_format_args(&json!({
            "paths": ["app.py"],
            "mode": "plan"
        }))
        .expect("request");

        let plan = plan_actions(&ws, &request).expect("plan");
        assert_eq!(plan.groups[0].adapter_id, "black");
        assert_eq!(
            plan.groups[0].config_path.as_deref(),
            Some("pyproject.toml")
        );
        assert_eq!(plan.files[0].selection_source, "manifest");
    }

    #[test]
    fn package_manifest_selects_declared_formatter() {
        let temp = tempdir().expect("workspace");
        fs::write(
            temp.path().join("package.json"),
            r#"{"devDependencies":{"prettier":"3.0.0"},"scripts":{"format":"prettier --write ."}}"#,
        )
        .expect("package manifest");
        fs::write(temp.path().join("app.ts"), "const x=1\n").expect("typescript");
        let ws = Workspace::new(temp.path().to_path_buf()).expect("workspace");
        let request = ActionRequest::from_format_args(&json!({
            "paths": ["app.ts"],
            "mode": "plan"
        }))
        .expect("request");

        let plan = plan_actions(&ws, &request).expect("plan");
        assert_eq!(plan.groups[0].adapter_id, "prettier");
        assert_eq!(plan.groups[0].config_path.as_deref(), Some("package.json"));
        assert_eq!(plan.files[0].selection_source, "manifest");
    }

    #[test]
    fn custom_formatter_plan_uses_workspace_configuration() {
        let temp = tempdir().expect("workspace");
        fs::create_dir_all(temp.path().join(".coding-tools")).expect("config dir");
        fs::create_dir_all(temp.path().join("tools")).expect("tools dir");
        fs::write(temp.path().join("tools/template-format"), "placeholder\n")
            .expect("formatter program");
        fs::write(
            temp.path().join(".coding-tools/formatters.json"),
            r#"{"formatters":{"company-template":{"program":"tools/template-format","extensions":["tmpl"],"args":["--write","{files}"]}}}"#,
        )
        .expect("formatter config");
        fs::write(temp.path().join("page.tmpl"), "hello\n").expect("template");
        let ws = Workspace::new(temp.path().to_path_buf()).expect("workspace");
        let request = ActionRequest::from_format_args(&json!({
            "paths": ["page.tmpl"],
            "mode": "plan",
            "formatter": "company-template"
        }))
        .expect("request");

        let plan = plan_actions(&ws, &request).expect("custom plan");
        assert_eq!(plan.groups[0].adapter_id, "company-template");
        assert_eq!(plan.files[0].selection_source, "workspace_config");
    }

    #[test]
    fn custom_formatter_rejects_programs_outside_workspace() {
        let temp = tempdir().expect("workspace");
        fs::create_dir_all(temp.path().join(".coding-tools")).expect("config dir");
        fs::write(
            temp.path().join(".coding-tools/formatters.json"),
            r#"{"formatters":{"unsafe":{"program":"../format","extensions":["tmpl"],"args":["{files}"]}}}"#,
        )
        .expect("formatter config");
        fs::write(temp.path().join("page.tmpl"), "hello\n").expect("template");
        let ws = Workspace::new(temp.path().to_path_buf()).expect("workspace");
        let request = ActionRequest::from_format_args(&json!({
            "paths": ["page.tmpl"],
            "mode": "plan",
            "formatter": "unsafe"
        }))
        .expect("request");

        let error = plan_actions(&ws, &request).expect_err("unsafe custom formatter");
        assert_eq!(error.to_error_value()["code"], "FORMATTER_CONFIG_INVALID");
    }

    #[test]
    fn custom_formatter_execution_requires_confirmation() {
        let temp = tempdir().expect("workspace");
        fs::create_dir_all(temp.path().join(".coding-tools")).expect("config dir");
        fs::create_dir_all(temp.path().join("tools")).expect("tools dir");
        fs::write(temp.path().join("tools/template-format"), "placeholder\n")
            .expect("formatter program");
        fs::write(
            temp.path().join(".coding-tools/formatters.json"),
            r#"{"formatters":{"company-template":{"program":"tools/template-format","extensions":["tmpl"],"args":["--write","{files}"]}}}"#,
        )
        .expect("formatter config");
        fs::write(temp.path().join("page.tmpl"), "hello\n").expect("template");
        let ws = Workspace::new(temp.path().to_path_buf()).expect("workspace");
        let request = ActionRequest::from_format_args(&json!({
            "paths": ["page.tmpl"],
            "mode": "check",
            "formatter": "company-template"
        }))
        .expect("request");

        let error = execute_actions_with_runner(&ws, &request, &UnavailableRunner)
            .expect_err("custom formatter confirmation");
        assert_eq!(
            error.to_error_value()["code"],
            "CUSTOM_FORMATTER_REQUIRES_CONFIRMATION"
        );
    }

    fn git(workspace: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(workspace)
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?} failed");
    }

    #[test]
    fn changed_and_staged_scopes_select_the_expected_git_files() {
        let temp = tempdir().expect("workspace");
        git(temp.path(), &["init"]);
        git(temp.path(), &["config", "user.name", "Format Test"]);
        git(
            temp.path(),
            &["config", "user.email", "format@example.invalid"],
        );
        fs::write(temp.path().join("staged.json"), "{\"a\":1}\n").expect("staged");
        fs::write(temp.path().join("working.json"), "{\"b\":2}\n").expect("working");
        fs::write(temp.path().join("unchanged.json"), "{\"c\":3}\n").expect("unchanged");
        git(temp.path(), &["add", "."]);
        git(temp.path(), &["commit", "-m", "baseline"]);

        fs::write(temp.path().join("staged.json"), "{\"a\": 10}\n").expect("modify staged");
        git(temp.path(), &["add", "staged.json"]);
        fs::write(temp.path().join("working.json"), "{\"b\": 20}\n").expect("modify working");
        fs::write(temp.path().join("untracked.json"), "{\"d\":4}\n").expect("untracked");

        let ws = Workspace::new(temp.path().to_path_buf()).expect("workspace");
        let changed = ActionRequest::from_format_args(&json!({
            "scope": "changed",
            "mode": "plan"
        }))
        .expect("changed request");
        let staged = ActionRequest::from_format_args(&json!({
            "scope": "staged",
            "mode": "plan"
        }))
        .expect("staged request");

        let changed_plan = plan_actions(&ws, &changed).expect("changed plan");
        let changed_paths = changed_plan
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            changed_paths,
            vec!["staged.json", "untracked.json", "working.json"]
        );

        let staged_plan = plan_actions(&ws, &staged).expect("staged plan");
        let staged_paths = staged_plan
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(staged_paths, vec!["staged.json"]);
    }

    #[test]
    fn adapter_commands_are_structured_and_never_use_shell_syntax() {
        let cases = [
            (
                "rustfmt",
                vec!["main.rs"],
                vec!["--edition", "2021", "main.rs"],
            ),
            ("prettier", vec!["app.ts"], vec!["--write", "app.ts"]),
            ("biome", vec!["app.ts"], vec!["format", "--write", "app.ts"]),
            ("ruff", vec!["app.py"], vec!["format", "app.py"]),
            ("gofmt", vec!["main.go"], vec!["-w", "main.go"]),
            ("clang-format", vec!["main.cpp"], vec!["-i", "main.cpp"]),
            ("shfmt", vec!["run.sh"], vec!["-w", "run.sh"]),
            ("terraform-fmt", vec!["main.tf"], vec!["fmt", "main.tf"]),
            ("taplo", vec!["Cargo.toml"], vec!["format", "Cargo.toml"]),
        ];

        for (adapter_id, files, expected_args) in cases {
            let request = RunnerRequest {
                adapter_id: adapter_id.to_string(),
                mirror_root: std::path::PathBuf::from("mirror"),
                files: files.into_iter().map(str::to_string).collect(),
                config_path: None,
                rust_edition: None,
                timeout_ms: 1_000,
                command_override: None,
            };
            let command = build_adapter_command(&request).expect("command");
            assert!(!command.executable_candidates.is_empty(), "{adapter_id}");
            assert_eq!(command.args, expected_args, "{adapter_id}");
            assert!(command
                .args
                .iter()
                .all(|argument| !argument.contains("&&") && !argument.contains('|')));
        }
    }

    struct UnavailableRunner;

    impl ActionRunner for UnavailableRunner {
        fn run(
            &self,
            request: &RunnerRequest,
        ) -> Result<RunnerOutput, crate::tools::workspace::WorkspaceError> {
            Err(crate::tools::workspace::WorkspaceError::ToolDetails {
                code: "FORMATTER_UNAVAILABLE",
                message: format!("Formatter {} is unavailable", request.adapter_id),
                category: "runtime",
                retryable: true,
                details: json!({"adapter_id": request.adapter_id}),
            })
        }
    }

    #[test]
    fn unavailable_formatter_is_skipped_when_not_strict() {
        let temp = tempdir().expect("workspace");
        fs::write(temp.path().join("main.rs"), "fn main(){}\n").expect("rust");
        let ws = Workspace::new(temp.path().to_path_buf()).expect("workspace");
        let request = ActionRequest::from_format_args(&json!({
            "paths": ["main.rs"],
            "mode": "check"
        }))
        .expect("request");

        let outcome = execute_actions_with_runner(&ws, &request, &UnavailableRunner)
            .expect("non-strict unavailable formatter");
        assert_eq!(outcome.unavailable_adapters, vec!["rustfmt"]);
        assert!(outcome
            .files_skipped
            .iter()
            .any(|file| file.path == "main.rs" && file.reason == "formatter_unavailable"));
        assert!(outcome.files_changed.is_empty());
    }

    #[test]
    fn unavailable_formatter_fails_in_strict_mode() {
        let temp = tempdir().expect("workspace");
        fs::write(temp.path().join("main.rs"), "fn main(){}\n").expect("rust");
        let ws = Workspace::new(temp.path().to_path_buf()).expect("workspace");
        let request = ActionRequest::from_format_args(&json!({
            "paths": ["main.rs"],
            "mode": "check",
            "strict": true
        }))
        .expect("request");

        let error = execute_actions_with_runner(&ws, &request, &UnavailableRunner)
            .expect_err("strict unavailable formatter");
        assert_eq!(error.to_error_value()["code"], "FORMATTER_UNAVAILABLE");
    }

    #[test]
    fn canonical_workspace_file_rejects_files_outside_workspace() {
        let workspace = tempdir().expect("workspace");
        let outside = tempdir().expect("outside");
        let inside_file = workspace.path().join("formatter");
        let outside_file = outside.path().join("formatter");
        fs::write(&inside_file, "inside\n").expect("inside formatter");
        fs::write(&outside_file, "outside\n").expect("outside formatter");

        assert_eq!(
            super::canonical_workspace_file(workspace.path(), &inside_file),
            Some(inside_file.canonicalize().expect("inside canonical"))
        );
        assert_eq!(
            super::canonical_workspace_file(workspace.path(), &outside_file),
            None
        );
    }

    #[test]
    fn executable_resolution_prefers_workspace_local_tools() {
        let temp = tempdir().expect("workspace");
        let bin = temp.path().join("node_modules/.bin");
        fs::create_dir_all(&bin).expect("bin");
        let executable = if cfg!(windows) {
            bin.join("prettier.cmd")
        } else {
            bin.join("prettier")
        };
        fs::write(&executable, "placeholder\n").expect("formatter");

        let resolved = resolve_formatter_executable(temp.path(), &["prettier".into()])
            .expect("workspace formatter");
        assert_eq!(
            resolved,
            executable.canonicalize().expect("formatter canonical path")
        );
    }

    struct PanicRunner;

    impl ActionRunner for PanicRunner {
        fn run(
            &self,
            _request: &RunnerRequest,
        ) -> Result<RunnerOutput, crate::tools::workspace::WorkspaceError> {
            panic!("builtin formatter must not invoke the external runner")
        }
    }

    struct RustFormattingRunner;

    impl ActionRunner for RustFormattingRunner {
        fn run(
            &self,
            request: &RunnerRequest,
        ) -> Result<RunnerOutput, crate::tools::workspace::WorkspaceError> {
            assert_eq!(request.adapter_id, "rustfmt");
            for path in &request.files {
                fs::write(request.mirror_root.join(path), "fn main() {}\n")
                    .expect("format mirror file");
            }
            Ok(RunnerOutput {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    struct UnexpectedChangeRunner;

    impl ActionRunner for UnexpectedChangeRunner {
        fn run(
            &self,
            request: &RunnerRequest,
        ) -> Result<RunnerOutput, crate::tools::workspace::WorkspaceError> {
            for path in &request.files {
                fs::write(request.mirror_root.join(path), "fn main() {}\n")
                    .expect("format mirror file");
            }
            fs::write(request.mirror_root.join("unexpected.txt"), "surprise\n")
                .expect("unexpected file");
            Ok(RunnerOutput {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    #[test]
    fn check_formats_in_isolation_without_writing_workspace() {
        let temp = tempdir().expect("workspace");
        fs::write(temp.path().join("data.json"), "{\"b\":2,\"a\":1}\n").expect("json");
        let ws = Workspace::new(temp.path().to_path_buf()).expect("workspace");
        let request = ActionRequest::from_format_args(&json!({
            "paths": ["data.json"],
            "mode": "check"
        }))
        .expect("request");

        let outcome = execute_actions_with_runner(&ws, &request, &PanicRunner).expect("check");
        assert_eq!(outcome.files_changed, vec!["data.json"]);
        assert!(!outcome.applied);
        assert!(outcome.diff.contains("data.json"));
        assert_eq!(
            fs::read_to_string(temp.path().join("data.json")).expect("read json"),
            "{\"b\":2,\"a\":1}\n"
        );
    }

    #[test]
    fn apply_writes_builtin_formatter_output_after_guarded_preflight() {
        let temp = tempdir().expect("workspace");
        fs::write(temp.path().join("data.json"), "{\"b\":2,\"a\":1}\n").expect("json");
        let ws = Workspace::new(temp.path().to_path_buf()).expect("workspace");
        let request = ActionRequest::from_format_args(&json!({
            "paths": ["data.json"],
            "mode": "apply"
        }))
        .expect("request");

        let outcome = execute_actions_with_runner(&ws, &request, &PanicRunner).expect("apply");
        assert!(outcome.applied);
        assert_eq!(outcome.files_changed, vec!["data.json"]);
        assert_eq!(
            fs::read_to_string(temp.path().join("data.json")).expect("read json"),
            "{\n  \"a\": 1,\n  \"b\": 2\n}\n"
        );
    }

    #[test]
    fn apply_rejects_expected_sha_mismatch_without_writing() {
        let temp = tempdir().expect("workspace");
        fs::write(temp.path().join("data.json"), "{\"a\":1}\n").expect("json");
        let ws = Workspace::new(temp.path().to_path_buf()).expect("workspace");
        let request = ActionRequest::from_format_args(&json!({
            "paths": ["data.json"],
            "mode": "apply",
            "expected_sha256": {
                "data.json": "0000000000000000000000000000000000000000000000000000000000000000"
            }
        }))
        .expect("request");

        let error = execute_actions_with_runner(&ws, &request, &PanicRunner).expect_err("mismatch");
        assert_eq!(error.to_error_value()["code"], "FILE_VERSION_MISMATCH");
        assert_eq!(
            fs::read_to_string(temp.path().join("data.json")).expect("read json"),
            "{\"a\":1}\n"
        );
    }

    #[test]
    fn external_runner_formats_only_the_isolated_mirror_then_applies() {
        let temp = tempdir().expect("workspace");
        fs::write(temp.path().join("main.rs"), "fn main(){}\n").expect("rust");
        let ws = Workspace::new(temp.path().to_path_buf()).expect("workspace");
        let request = ActionRequest::from_format_args(&json!({
            "paths": ["main.rs"],
            "mode": "apply"
        }))
        .expect("request");

        let outcome =
            execute_actions_with_runner(&ws, &request, &RustFormattingRunner).expect("apply");
        assert!(outcome.applied);
        assert_eq!(
            fs::read_to_string(temp.path().join("main.rs")).expect("read rust"),
            "fn main() {}\n"
        );
    }

    #[test]
    fn unexpected_mirror_changes_abort_without_touching_workspace() {
        let temp = tempdir().expect("workspace");
        fs::write(temp.path().join("main.rs"), "fn main(){}\n").expect("rust");
        let ws = Workspace::new(temp.path().to_path_buf()).expect("workspace");
        let request = ActionRequest::from_format_args(&json!({
            "paths": ["main.rs"],
            "mode": "apply"
        }))
        .expect("request");

        let error = execute_actions_with_runner(&ws, &request, &UnexpectedChangeRunner)
            .expect_err("unexpected change");
        assert_eq!(error.to_error_value()["code"], "FORMAT_UNEXPECTED_CHANGES");
        assert_eq!(
            fs::read_to_string(temp.path().join("main.rs")).expect("read rust"),
            "fn main(){}\n"
        );
    }
}
