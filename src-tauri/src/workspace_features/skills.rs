use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use walkdir::WalkDir;

use crate::workspace::WorkspaceFolder;

const MAX_SKILL_BYTES: u64 = 256 * 1024;
const MAX_SKILL_FILES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillEvolution {
    pub knowledge_id: String,
    pub generation: u64,
    pub base_content_sha256: String,
}

#[derive(Debug, Clone)]
pub struct SkillDescriptor {
    pub key: String,
    pub name: String,
    pub description: String,
    pub source: String,
    pub scope: String,
    pub relative_path: String,
    pub root_relative_path: String,
    pub version: Option<String>,
    pub content: String,
    pub body: String,
    pub folder_id: Option<String>,
    pub folder_name: Option<String>,
    pub evolution: Option<SkillEvolution>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillDiagnostic {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

pub struct DiscoveredSkills {
    pub skills: Vec<SkillDescriptor>,
    pub diagnostics: Vec<SkillDiagnostic>,
}

struct RootSpec {
    root: PathBuf,
    containment: PathBuf,
    display_base: PathBuf,
    display_prefix: Option<String>,
    source: &'static str,
    scope: &'static str,
    precedence: usize,
    max_depth: usize,
    folder_id: Option<String>,
    folder_name: Option<String>,
}

fn normalize_slashes(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn display_path(root: &RootSpec, candidate: &Path) -> String {
    if root.scope == "user" {
        let relative = candidate
            .strip_prefix(&root.root)
            .ok()
            .map(normalize_slashes)
            .unwrap_or_default();
        if relative.is_empty() {
            root.display_prefix.clone().unwrap_or_else(|| "~".into())
        } else {
            format!(
                "{}/{}",
                root.display_prefix.as_deref().unwrap_or("~"),
                relative
            )
        }
    } else {
        candidate
            .strip_prefix(&root.display_base)
            .ok()
            .map(normalize_slashes)
            .unwrap_or_else(|| normalize_slashes(candidate))
    }
}

fn control_key(root: &RootSpec, relative_path: &str) -> String {
    if root.scope == "user" {
        format!("user:{}:{}", root.source, relative_path)
    } else {
        format!(
            "workspace:{}:{}:{}",
            root.folder_id.as_deref().unwrap_or("workspace"),
            root.source,
            relative_path
        )
    }
}

fn unquote(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
        return serde_json::from_str::<String>(trimmed)
            .map_err(|_| "invalid double-quoted YAML scalar".to_string());
    }
    if trimmed.starts_with('\'') && trimmed.ends_with('\'') && trimmed.len() >= 2 {
        return Ok(trimmed[1..trimmed.len() - 1].replace("''", "'"));
    }
    Ok(trimmed.to_string())
}

fn parse_frontmatter(lines: &[&str]) -> Result<HashMap<String, String>, String> {
    let mut values = HashMap::new();
    let mut index = 0usize;
    while index < lines.len() {
        let line = lines[index];
        let trimmed = line.trim();
        if trimmed.is_empty() || line.trim_start().starts_with('#') {
            index += 1;
            continue;
        }
        let Some((raw_key, raw_value)) = line.split_once(':') else {
            index += 1;
            continue;
        };
        if raw_key.is_empty()
            || !raw_key
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-'))
        {
            index += 1;
            continue;
        }
        let key = raw_key.to_ascii_lowercase();
        let raw = raw_value.trim();
        if raw == "|" || raw == ">" {
            let mut block = Vec::new();
            index += 1;
            while index < lines.len() {
                let nested = lines[index];
                if nested
                    .split_once(':')
                    .map(|(key, _)| {
                        !key.is_empty()
                            && key.chars().all(|ch| {
                                ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-')
                            })
                    })
                    .unwrap_or(false)
                    && !nested.starts_with(' ')
                    && !nested.starts_with('\t')
                {
                    break;
                }
                if nested.starts_with(' ') || nested.starts_with('\t') || nested.trim().is_empty() {
                    block.push(nested.trim_start_matches(' ').trim_start_matches('\t'));
                    index += 1;
                } else {
                    break;
                }
            }
            values.insert(
                key,
                if raw == ">" {
                    block.join(" ").trim().to_string()
                } else {
                    block.join("\n").trim().to_string()
                },
            );
            continue;
        }
        values.insert(key, unquote(raw)?);
        index += 1;
    }
    Ok(values)
}

fn parse_skill(input: &str) -> Result<(String, String, String), String> {
    let normalized = input.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let lines = normalized.split('\n').collect::<Vec<_>>();
    if lines.first().map(|line| line.trim()) != Some("---") {
        return Err("SKILL.md must start with YAML frontmatter".into());
    }
    let closing = lines
        .iter()
        .enumerate()
        .skip(1)
        .take(256)
        .find_map(|(index, line)| (line.trim() == "---").then_some(index))
        .ok_or_else(|| "SKILL.md frontmatter is not terminated".to_string())?;
    let frontmatter = parse_frontmatter(&lines[1..closing])?;
    let name = frontmatter
        .get("name")
        .cloned()
        .unwrap_or_default()
        .trim()
        .to_string();
    let description = frontmatter
        .get("description")
        .cloned()
        .unwrap_or_default()
        .trim()
        .to_string();
    if name.is_empty() {
        return Err("SKILL.md frontmatter requires name".into());
    }
    if description.is_empty() {
        return Err("SKILL.md frontmatter requires description".into());
    }
    if name.chars().count() > 160 {
        return Err("SKILL.md name is too long".into());
    }
    if description.chars().count() > 4096 {
        return Err("SKILL.md description is too long".into());
    }
    let body = lines[closing + 1..].join("\n").trim().to_string();
    Ok((name, description, body))
}

fn read_version(skill_root: &Path, containment: &Path) -> Option<String> {
    let path = skill_root.join("VERSION");
    let metadata = fs::symlink_metadata(&path).ok()?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() || metadata.len() > 256
    {
        return None;
    }
    let resolved = path.canonicalize().ok()?;
    if !resolved.starts_with(containment) {
        return None;
    }
    let version = fs::read_to_string(resolved).ok()?.trim().to_string();
    (!version.is_empty()).then_some(version)
}

fn discover_root(
    root: &RootSpec,
    diagnostics: &mut Vec<SkillDiagnostic>,
) -> Vec<(usize, SkillDescriptor)> {
    if !root.root.exists() {
        return Vec::new();
    }
    let mut result = Vec::new();
    for entry in WalkDir::new(&root.root)
        .follow_links(false)
        .max_depth(root.max_depth + 1)
        .sort_by_file_name()
        .into_iter()
    {
        if result.len() >= MAX_SKILL_FILES {
            diagnostics.push(SkillDiagnostic {
                code: "SKILL_DISCOVERY_LIMIT_REACHED".into(),
                message: format!(
                    "Skill discovery is limited to {MAX_SKILL_FILES} SKILL.md files per workspace."
                ),
                path: None,
                name: None,
                source: None,
                scope: None,
            });
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                diagnostics.push(SkillDiagnostic {
                    code: "SKILL_DISCOVERY_FAILED".into(),
                    message: error.to_string(),
                    path: None,
                    name: None,
                    source: Some(root.source.into()),
                    scope: Some(root.scope.into()),
                });
                continue;
            }
        };
        if entry.file_type().is_symlink() {
            diagnostics.push(SkillDiagnostic {
                code: "SKILL_SYMLINK_SKIPPED".into(),
                message: "Skill discovery does not follow symlinks.".into(),
                path: Some(display_path(root, entry.path())),
                name: None,
                source: Some(root.source.into()),
                scope: Some(root.scope.into()),
            });
            continue;
        }
        if !entry.file_type().is_file()
            || !entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case("SKILL.md")
        {
            continue;
        }
        let path = entry.path();
        let display = display_path(root, path);
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) => {
                diagnostics.push(SkillDiagnostic {
                    code: "SKILL_INVALID".into(),
                    message: error.to_string(),
                    path: Some(display),
                    name: None,
                    source: Some(root.source.into()),
                    scope: Some(root.scope.into()),
                });
                continue;
            }
        };
        if metadata.len() > MAX_SKILL_BYTES {
            diagnostics.push(SkillDiagnostic {
                code: "SKILL_TOO_LARGE".into(),
                message: format!("SKILL.md exceeds {MAX_SKILL_BYTES} bytes."),
                path: Some(display),
                name: None,
                source: Some(root.source.into()),
                scope: Some(root.scope.into()),
            });
            continue;
        }
        let resolved = match path.canonicalize() {
            Ok(resolved) => resolved,
            Err(error) => {
                diagnostics.push(SkillDiagnostic {
                    code: "SKILL_INVALID".into(),
                    message: error.to_string(),
                    path: Some(display),
                    name: None,
                    source: Some(root.source.into()),
                    scope: Some(root.scope.into()),
                });
                continue;
            }
        };
        if !resolved.starts_with(&root.containment) {
            diagnostics.push(SkillDiagnostic {
                code: if root.scope == "workspace" {
                    "SKILL_OUTSIDE_WORKSPACE".into()
                } else {
                    "SKILL_OUTSIDE_USER_HOME".into()
                },
                message: if root.scope == "workspace" {
                    "Resolved SKILL.md escapes the configured workspace.".into()
                } else {
                    "Resolved user-level SKILL.md escapes the configured user home.".into()
                },
                path: Some(display),
                name: None,
                source: Some(root.source.into()),
                scope: Some(root.scope.into()),
            });
            continue;
        }
        let content = match fs::read_to_string(&resolved) {
            Ok(content) => content,
            Err(error) => {
                diagnostics.push(SkillDiagnostic {
                    code: "SKILL_INVALID".into(),
                    message: error.to_string(),
                    path: Some(display),
                    name: None,
                    source: Some(root.source.into()),
                    scope: Some(root.scope.into()),
                });
                continue;
            }
        };
        let (name, description, body) = match parse_skill(&content) {
            Ok(parsed) => parsed,
            Err(message) => {
                diagnostics.push(SkillDiagnostic {
                    code: "SKILL_INVALID".into(),
                    message,
                    path: Some(display),
                    name: None,
                    source: Some(root.source.into()),
                    scope: Some(root.scope.into()),
                });
                continue;
            }
        };
        let skill_root = resolved.parent().unwrap_or(&resolved);
        let root_display = display_path(root, skill_root);
        result.push((
            root.precedence,
            SkillDescriptor {
                key: control_key(root, &display),
                name,
                description,
                source: root.source.into(),
                scope: root.scope.into(),
                relative_path: display,
                root_relative_path: root_display,
                version: read_version(skill_root, &root.containment),
                content,
                body,
                folder_id: root.folder_id.clone(),
                folder_name: root.folder_name.clone(),
                evolution: None,
            },
        ));
    }
    result
}

fn roots_for_folder(folder: &WorkspaceFolder) -> Result<Vec<RootSpec>, SkillDiagnostic> {
    let workspace = PathBuf::from(&folder.path);
    let workspace_real = workspace.canonicalize().map_err(|error| SkillDiagnostic {
        code: "SKILL_DISCOVERY_FAILED".into(),
        message: error.to_string(),
        path: Some(folder.path.clone()),
        name: None,
        source: None,
        scope: Some("workspace".into()),
    })?;
    let mut roots = vec![
        RootSpec {
            root: workspace.join("skills"),
            containment: workspace_real.clone(),
            display_base: workspace.clone(),
            display_prefix: None,
            source: "project",
            scope: "workspace",
            precedence: 0,
            max_depth: 3,
            folder_id: Some(folder.id.clone()),
            folder_name: Some(folder.name.clone()),
        },
        RootSpec {
            root: workspace.join(".agents").join("skills"),
            containment: workspace_real.clone(),
            display_base: workspace.clone(),
            display_prefix: None,
            source: "agents",
            scope: "workspace",
            precedence: 10,
            max_depth: 5,
            folder_id: Some(folder.id.clone()),
            folder_name: Some(folder.name.clone()),
        },
        RootSpec {
            root: workspace.join(".claude").join("skills"),
            containment: workspace_real,
            display_base: workspace.clone(),
            display_prefix: None,
            source: "claude",
            scope: "workspace",
            precedence: 20,
            max_depth: 7,
            folder_id: Some(folder.id.clone()),
            folder_name: Some(folder.name.clone()),
        },
    ];
    if let Some(home) = dirs::home_dir() {
        if let Ok(home_real) = home.canonicalize() {
            roots.extend([
                RootSpec {
                    root: home.join(".agents").join("skills"),
                    containment: home_real.clone(),
                    display_base: home.join(".agents").join("skills"),
                    display_prefix: Some("~/.agents/skills".into()),
                    source: "codex-user",
                    scope: "user",
                    precedence: 100,
                    max_depth: 5,
                    folder_id: None,
                    folder_name: None,
                },
                RootSpec {
                    root: home.join(".claude").join("skills"),
                    containment: home_real,
                    display_base: home.join(".claude").join("skills"),
                    display_prefix: Some("~/.claude/skills".into()),
                    source: "claude-user",
                    scope: "user",
                    precedence: 110,
                    max_depth: 7,
                    folder_id: None,
                    folder_name: None,
                },
            ]);
        }
    }
    Ok(roots)
}

pub fn discover_skills(folders: &[WorkspaceFolder]) -> DiscoveredSkills {
    let mut diagnostics = Vec::new();
    let mut combined = Vec::<SkillDescriptor>::new();
    let mut seen_keys = HashSet::new();
    for folder in folders {
        let roots = match roots_for_folder(folder) {
            Ok(roots) => roots,
            Err(diagnostic) => {
                diagnostics.push(diagnostic);
                continue;
            }
        };
        let mut candidates = Vec::new();
        for root in roots {
            candidates.extend(discover_root(&root, &mut diagnostics));
        }
        candidates.sort_by(|(left_precedence, left), (right_precedence, right)| {
            left_precedence
                .cmp(right_precedence)
                .then_with(|| left.relative_path.cmp(&right.relative_path))
                .then_with(|| left.name.cmp(&right.name))
        });
        let mut selected_names = HashMap::<String, SkillDescriptor>::new();
        for (_, skill) in candidates {
            let identity = skill.name.to_ascii_lowercase();
            if let Some(existing) = selected_names.get(&identity) {
                diagnostics.push(SkillDiagnostic {
                    code: "SKILL_SHADOWED".into(),
                    message: format!(
                        "{} is shadowed by {}.",
                        skill.relative_path, existing.relative_path
                    ),
                    path: Some(skill.relative_path.clone()),
                    name: Some(skill.name.clone()),
                    source: Some(skill.source.clone()),
                    scope: Some(skill.scope.clone()),
                });
            } else {
                selected_names.insert(identity, skill);
            }
        }
        let mut selected = selected_names.into_values().collect::<Vec<_>>();
        selected.sort_by(|left, right| left.name.cmp(&right.name));
        for skill in selected {
            if seen_keys.insert(skill.key.clone()) {
                combined.push(skill);
            }
        }
    }
    combined.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.key.cmp(&right.key))
    });
    DiscoveredSkills {
        skills: combined,
        diagnostics,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn discovers_workspace_skill_with_node_compatible_key() {
        let root = tempdir().expect("tempdir");
        let skill = root.path().join(".agents/skills/example");
        fs::create_dir_all(&skill).expect("mkdir");
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: example\ndescription: Example skill\n---\nBody\n",
        )
        .expect("write");
        let folder = WorkspaceFolder::new(
            root.path().to_string_lossy().into_owned(),
            Some("root".into()),
        );
        let found = discover_skills(&[folder.clone()]);
        let example = found
            .skills
            .iter()
            .find(|skill| {
                skill.name == "example" && skill.folder_id.as_deref() == Some(folder.id.as_str())
            })
            .expect("workspace example skill");
        assert_eq!(example.scope, "workspace");
        assert_eq!(example.source, "agents");
        assert_eq!(
            example.key,
            format!(
                "workspace:{}:agents:.agents/skills/example/SKILL.md",
                folder.id
            )
        );
    }
}
