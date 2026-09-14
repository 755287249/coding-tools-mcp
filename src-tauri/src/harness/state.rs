use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;
use walkdir::WalkDir;

use crate::tools::dispatch::{fast_containing_git_root, fast_workspace_git_identity};

use super::model::{
    BaselineEntry, CapabilityStatus, ChangeSet, FileChangeRecord, HarnessEvent, HarnessStatus,
    OperationRecord, ProjectBaseline, ProjectFileState, ProjectState, ReasonRecord, TaskSession,
    TaskStatus, WorkspaceHarnessState, SCHEMA_VERSION,
};
use super::store::{HarnessError, HarnessResult, HarnessStore};

#[derive(Debug, Clone)]
pub struct Harness {
    workspace_root: PathBuf,
    workspace_id: String,
    store: HarnessStore,
}

impl Harness {
    pub fn new(workspace_root: PathBuf, harness_root: PathBuf) -> HarnessResult<Self> {
        let workspace_root = workspace_root
            .canonicalize()
            .map_err(|e| HarnessError::new("WORKSPACE_UNAVAILABLE", e.to_string()))?;
        let workspace_id = workspace_id(&workspace_root);
        Ok(Self {
            workspace_root,
            workspace_id,
            store: HarnessStore::new(harness_root)?,
        })
    }

    pub fn default_root() -> HarnessResult<PathBuf> {
        let root = dirs::data_local_dir()
            .or_else(dirs::data_dir)
            .ok_or_else(|| HarnessError::new("STORE_UNAVAILABLE", "无法确定应用数据目录"))?;
        Ok(root.join("coding-tools-mcp").join("harness"))
    }

    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    pub fn store_root(&self) -> &Path {
        self.store.root()
    }

    pub fn start_task(&self, objective: &str) -> HarnessResult<TaskSession> {
        self.start_task_for(objective, &self.workspace_root)
    }

    pub fn start_task_for(&self, objective: &str, scope_root: &Path) -> HarnessResult<TaskSession> {
        if objective.trim().is_empty() {
            return Err(HarnessError::new("INVALID_ARGUMENT", "任务目标不能为空"));
        }
        let scope_root = self.normalize_scope_root(scope_root);
        let scope_id = self.scope_id_for_root(&scope_root);
        if let Some(task) = self.current_task_for_scope(&scope_id)? {
            return Err(HarnessError::new(
                "TASK_ALREADY_ACTIVE",
                format!("当前工作树已有活动任务 {}", task.id),
            ));
        }
        let baseline = capture_baseline(&scope_root);
        let now = timestamp();
        let task = TaskSession {
            id: Uuid::new_v4().simple().to_string(),
            workspace_id: self.workspace_id.clone(),
            scope_id: Some(scope_id),
            scope_root: Some(scope_root.to_string_lossy().to_string()),
            objective: objective.trim().to_string(),
            status: TaskStatus::Active,
            expected_fingerprint: baseline.worktree_fingerprint.clone(),
            baseline,
            completed_steps: Vec::new(),
            pending_steps: Vec::new(),
            latest_change_id: None,
            latest_verification_id: None,
            created_at: now.clone(),
            updated_at: now,
        };
        self.store.save_task(&task)?;
        self.save_workspace_state(Some(&task.id), &task)?;
        self.record_event(
            &task.id,
            "task_started",
            None,
            json!({}),
            json!({"ok": true}),
        )?;
        Ok(task)
    }

    pub fn current_task(&self) -> HarnessResult<Option<TaskSession>> {
        self.current_task_for_scope(&self.workspace_id)
    }

    pub fn current_task_for_root(&self, scope_root: &Path) -> HarnessResult<Option<TaskSession>> {
        let scope_root = self.normalize_scope_root(scope_root);
        let scope_id = self.scope_id_for_root(&scope_root);
        self.current_task_for_scope(&scope_id)
    }

    pub fn current_task_for_scope(&self, scope_id: &str) -> HarnessResult<Option<TaskSession>> {
        let mut state = match self.store.load_workspace_state(&self.workspace_id)? {
            Some(state) if state.active_task_ids.is_some() => state,
            state => self.repair_workspace_state(state)?,
        };
        if let Some(task_id) = state
            .active_task_ids
            .as_ref()
            .and_then(|active| active.get(scope_id))
            .cloned()
        {
            if let Ok(task) = self.store.load_task(&self.workspace_id, &task_id) {
                if task.status.is_writable() && self.task_scope_id(&task) == scope_id {
                    return Ok(Some(task));
                }
            }
            state = self.repair_workspace_state(Some(state))?;
        } else if self
            .store
            .workspace_state_covers_task_entries(&self.workspace_id)
        {
            return Ok(None);
        } else {
            state = self.repair_workspace_state(Some(state))?;
        }
        let Some(task_id) = state
            .active_task_ids
            .as_ref()
            .and_then(|active| active.get(scope_id))
        else {
            return Ok(None);
        };
        let task = self.store.load_task(&self.workspace_id, task_id)?;
        Ok((task.status.is_writable() && self.task_scope_id(&task) == scope_id).then_some(task))
    }

    pub fn scope_root_for(&self, cwd: &Path) -> PathBuf {
        let cwd = cwd
            .canonicalize()
            .unwrap_or_else(|_| self.workspace_root.clone());
        let candidate = fast_containing_git_root(&self.workspace_root, &cwd)
            .or_else(|| {
                git_value(&cwd, &["rev-parse", "--show-toplevel"])
                    .map(PathBuf::from)
                    .and_then(|path| path.canonicalize().ok())
            })
            .unwrap_or_else(|| self.workspace_root.clone());
        self.normalize_scope_root(&candidate)
    }

    pub fn scope_id_for_root(&self, scope_root: &Path) -> String {
        let scope_root = self.normalize_scope_root(scope_root);
        if scope_root == self.workspace_root {
            self.workspace_id.clone()
        } else {
            workspace_id(&scope_root)
        }
    }

    fn normalize_scope_root(&self, scope_root: &Path) -> PathBuf {
        let candidate = scope_root
            .canonicalize()
            .unwrap_or_else(|_| self.workspace_root.clone());
        if candidate == self.workspace_root || candidate.strip_prefix(&self.workspace_root).is_ok()
        {
            candidate
        } else {
            self.workspace_root.clone()
        }
    }

    fn task_scope_id<'a>(&'a self, task: &'a TaskSession) -> &'a str {
        task.scope_id.as_deref().unwrap_or(&task.workspace_id)
    }

    fn task_root(&self, task: &TaskSession) -> PathBuf {
        task.scope_root
            .as_deref()
            .map(PathBuf::from)
            .and_then(|path| path.canonicalize().ok())
            .filter(|path| {
                path == &self.workspace_root || path.strip_prefix(&self.workspace_root).is_ok()
            })
            .unwrap_or_else(|| self.workspace_root.clone())
    }

    pub fn task(&self, task_id: &str) -> HarnessResult<TaskSession> {
        self.store.load_task(&self.workspace_id, task_id)
    }

    pub fn change(&self, change_id: &str) -> HarnessResult<ChangeSet> {
        if !is_change_id(change_id) {
            return Err(HarnessError::new(
                "INVALID_ARGUMENT",
                "change_id 必须是 32 位小写十六进制 ID",
            ));
        }
        self.store.load_change(&self.workspace_id, change_id)
    }

    pub fn change_files(&self, task_id: &str) -> HarnessResult<Vec<FileChangeRecord>> {
        let task = self.task(task_id)?;
        Ok(change_records(
            &task.baseline,
            &capture_baseline(&self.task_root(&task)),
        ))
    }

    pub fn finish_task(
        &self,
        task_id: &str,
        summary: Option<&str>,
        next: TaskStatus,
    ) -> HarnessResult<(TaskSession, ChangeSet)> {
        let mut task = self.task(task_id)?;
        if !task.status.can_transition_to(next) {
            return Err(HarnessError::new(
                "INVALID_TASK_TRANSITION",
                format!("不允许从 {:?} 转换到 {:?}", task.status, next),
            ));
        }
        let reason = match summary {
            Some(value) if value.trim().is_empty() => {
                return Err(HarnessError::new("INVALID_ARGUMENT", "summary 不能为空"));
            }
            Some(value) => ReasonRecord {
                text: value.trim().to_string(),
                source: "finish_task_summary".to_string(),
            },
            None => ReasonRecord {
                text: task.objective.clone(),
                source: "task_objective".to_string(),
            },
        };
        let now = timestamp();
        let change_id = Uuid::new_v4().simple().to_string();
        let finished_event = HarnessEvent {
            id: Uuid::new_v4().simple().to_string(),
            task_id: task.id.clone(),
            operation_id: Uuid::new_v4().simple().to_string(),
            kind: "task_finished".to_string(),
            tool_name: None,
            input_summary: json!({
                "workspace_id": self.workspace_id,
                "payload": {"summary": reason.text, "status": next, "change_id": change_id}
            }),
            result_summary: json!({"ok": true, "status": next, "change_id": change_id}),
            reason: Some(reason.clone()),
            affected_files: Vec::new(),
            created_at: now.clone(),
        };
        let mut command_ids = self
            .list_events(task_id, 0, 2_000)?
            .into_iter()
            .map(|event| event.operation_id)
            .collect::<Vec<_>>();
        command_ids.push(finished_event.operation_id.clone());
        let change = ChangeSet {
            id: change_id,
            task_id: task.id.clone(),
            objective: task.objective.clone(),
            reason,
            files: change_records(&task.baseline, &capture_baseline(&self.task_root(&task))),
            command_ids,
            verification_ids: Vec::new(),
            risks: Vec::new(),
            created_at: now.clone(),
        };
        task.status = next;
        task.latest_change_id = Some(change.id.clone());
        task.updated_at = now;
        self.store.save_change(&self.workspace_id, &change)?;
        self.store.save_task(&task)?;
        self.store
            .append_event_for_workspace(&self.workspace_id, &finished_event)?;
        self.save_workspace_state(task.status.is_writable().then_some(task.id.as_str()), &task)?;
        Ok((task, change))
    }

    pub fn transition(&self, task_id: &str, next: TaskStatus) -> HarnessResult<TaskSession> {
        let mut task = self.task(task_id)?;
        if !task.status.can_transition_to(next) {
            return Err(HarnessError::new(
                "INVALID_TASK_TRANSITION",
                format!("不允许从 {:?} 转换到 {:?}", task.status, next),
            ));
        }
        task.status = next;
        task.updated_at = timestamp();
        self.store.save_task(&task)?;
        if !task.status.is_writable() {
            self.save_workspace_state(None, &task)?;
        }
        self.record_event(
            task_id,
            "task_status_changed",
            None,
            json!({"status": next}),
            json!({"ok": true}),
        )?;
        Ok(task)
    }

    pub fn update_steps(
        &self,
        task_id: &str,
        completed_steps: Option<Vec<String>>,
        pending_steps: Option<Vec<String>>,
    ) -> HarnessResult<TaskSession> {
        let mut task = self.task(task_id)?;
        if let Some(steps) = completed_steps {
            task.completed_steps = steps;
        }
        if let Some(steps) = pending_steps {
            task.pending_steps = steps;
        }
        task.updated_at = timestamp();
        self.store.save_task(&task)?;
        self.record_event(
            task_id,
            "task_updated",
            None,
            json!({
                "completed_steps": task.completed_steps,
                "pending_steps": task.pending_steps
            }),
            json!({"ok": true}),
        )?;
        Ok(task)
    }

    pub fn check_baseline(&self, task_id: &str) -> HarnessResult<()> {
        let task = self.task(task_id)?;
        let current = capture_baseline(&self.task_root(&task));
        if current.branch != task.baseline.branch || current.head != task.baseline.head {
            return Err(HarnessError::new(
                "BASELINE_STALE",
                "Git 分支或 HEAD 已发生变化",
            ));
        }
        if current.worktree_fingerprint != task.expected_fingerprint {
            return Err(HarnessError::new(
                "FILE_CHANGED_EXTERNALLY",
                "工作区存在 Harness 未记录的外部文件变化",
            ));
        }
        Ok(())
    }

    pub fn original_baseline_matches(
        &self,
        task_id: &str,
    ) -> HarnessResult<(bool, ProjectBaseline)> {
        let task = self.task(task_id)?;
        let current = capture_baseline(&self.task_root(&task));
        let matches = current.branch == task.baseline.branch
            && current.head == task.baseline.head
            && current.worktree_fingerprint == task.baseline.worktree_fingerprint;
        Ok((matches, current))
    }

    pub fn refresh_expected_state(&self, task_id: &str) -> HarnessResult<TaskSession> {
        let mut task = self.task(task_id)?;
        task.expected_fingerprint = capture_baseline(&self.task_root(&task)).worktree_fingerprint;
        task.updated_at = timestamp();
        self.store.save_task(&task)?;
        Ok(task)
    }

    pub fn record_event(
        &self,
        task_id: &str,
        kind: &str,
        tool_name: Option<&str>,
        input_summary: serde_json::Value,
        result_summary: serde_json::Value,
    ) -> HarnessResult<HarnessEvent> {
        let event = HarnessEvent {
            id: Uuid::new_v4().simple().to_string(),
            task_id: task_id.to_string(),
            operation_id: Uuid::new_v4().simple().to_string(),
            kind: kind.to_string(),
            tool_name: tool_name.map(str::to_string),
            input_summary: json!({"workspace_id": self.workspace_id, "payload": input_summary}),
            result_summary,
            reason: None,
            affected_files: Vec::<FileChangeRecord>::new(),
            created_at: timestamp(),
        };
        self.store
            .append_event_for_workspace(&self.workspace_id, &event)?;
        Ok(event)
    }

    pub fn list_events(
        &self,
        task_id: &str,
        offset: usize,
        limit: usize,
    ) -> HarnessResult<Vec<HarnessEvent>> {
        self.store
            .list_events(&self.workspace_id, task_id, offset, limit)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_operation(
        &self,
        operation_id: Option<&str>,
        task_id: Option<&str>,
        tool: &str,
        kind: &str,
        input_summary: serde_json::Value,
        result_summary: serde_json::Value,
    ) -> HarnessResult<OperationRecord> {
        let reason = input_summary
            .get("reason")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let operation = OperationRecord {
            id: operation_id
                .map(str::to_string)
                .unwrap_or_else(|| Uuid::new_v4().simple().to_string()),
            workspace_id: self.workspace_id.clone(),
            task_id: task_id.map(str::to_string),
            tool: tool.to_string(),
            kind: kind.to_string(),
            input_summary,
            result_summary,
            reason,
            affected_files: Vec::new(),
            created_at: timestamp(),
        };
        self.store
            .append_operation(&self.workspace_id, &operation)?;
        Ok(operation)
    }

    pub fn list_operations(
        &self,
        offset: usize,
        limit: usize,
    ) -> HarnessResult<Vec<OperationRecord>> {
        self.store
            .list_operations(&self.workspace_id, offset, limit)
    }

    pub fn project_state(&self, max_files: usize) -> HarnessResult<ProjectState> {
        self.project_state_for(&self.workspace_root, max_files)
    }

    pub fn project_state_for(
        &self,
        scope_root: &Path,
        max_files: usize,
    ) -> HarnessResult<ProjectState> {
        let scope_root = self.normalize_scope_root(scope_root);
        let scope_id = self.scope_id_for_root(&scope_root);
        let current = capture_baseline(&scope_root);
        let task = self.current_task_for_scope(&scope_id)?;
        let baseline_map = task
            .as_ref()
            .map(|t| {
                t.baseline
                    .entries
                    .iter()
                    .map(|e| (e.path.clone(), e))
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default();
        let current_map: HashMap<_, _> = current
            .entries
            .iter()
            .map(|e| (e.path.clone(), e))
            .collect();
        let mut paths: Vec<String> = baseline_map
            .keys()
            .chain(current_map.keys())
            .cloned()
            .collect();
        paths.sort();
        paths.dedup();
        let total_files = paths.len();
        let files = paths
            .into_iter()
            .map(|path| {
                let before = baseline_map.get(&path).map(|e| e.sha256.clone());
                let entry = current_map.get(&path);
                let status = match (before, entry) {
                    (Some(before), Some(entry)) if before == entry.sha256 => "unchanged",
                    (Some(_), Some(_)) => "modified",
                    (Some(_), None) => "deleted",
                    (None, Some(_)) => "added",
                    (None, None) => "unknown",
                };
                ProjectFileState {
                    path,
                    status: status.to_string(),
                    sha256: entry.map(|e| e.sha256.clone()).unwrap_or_default(),
                    bytes: entry.map(|e| e.bytes).unwrap_or(0),
                }
            })
            .collect::<Vec<_>>();
        let clean = files.iter().all(|file| file.status == "unchanged");
        let truncated = files.len() > max_files.max(1);
        let files = files.into_iter().take(max_files.max(1)).collect::<Vec<_>>();
        let active_task_id = task.as_ref().map(|t| t.id.clone());
        let recent_events = task
            .as_ref()
            .and_then(|t| self.list_events(&t.id, 0, 100).ok())
            .map(|events| events.len())
            .unwrap_or(0);
        Ok(ProjectState {
            schema_version: SCHEMA_VERSION,
            workspace_id: self.workspace_id.clone(),
            scope_id,
            scope_root: scope_root.to_string_lossy().to_string(),
            branch: current.branch,
            head: current.head,
            clean,
            files,
            total_files,
            truncated,
            active_task_id,
            task,
            recent_events,
        })
    }

    pub fn status(&self) -> HarnessResult<HarnessStatus> {
        self.status_for(&self.workspace_root)
    }

    pub fn status_for(&self, scope_root: &Path) -> HarnessResult<HarnessStatus> {
        let scope_root = self.normalize_scope_root(scope_root);
        let scope_id = self.scope_id_for_root(&scope_root);
        let task = self.current_task_for_scope(&scope_id)?;
        let current = task.as_ref().map(|_| capture_baseline(&scope_root));
        let needs_identity = current
            .as_ref()
            .is_none_or(|baseline| baseline.branch.is_none() || baseline.head.is_none());
        let fallback_identity = needs_identity
            .then(|| git_identity(&scope_root))
            .unwrap_or((None, None));
        let branch = current
            .as_ref()
            .and_then(|baseline| baseline.branch.clone())
            .or(fallback_identity.0);
        let head = current
            .as_ref()
            .and_then(|baseline| baseline.head.clone())
            .or(fallback_identity.1);
        let current_head = head.clone();
        let task_baseline_head = task.as_ref().and_then(|task| task.baseline.head.clone());
        let (task_id, task_state, task_updated_at, writable, baseline_matches, reason) =
            match task.as_ref() {
                Some(task) => {
                    let current = current.as_ref().expect("active task baseline");
                    let matches = task.baseline.branch == current.branch
                        && task.baseline.head == current.head
                        && task.expected_fingerprint == current.worktree_fingerprint;
                    let reason = if matches {
                        "任务可继续执行"
                    } else {
                        "工作区基线已变化，写入和执行已暂停"
                    };
                    (
                        Some(task.id.clone()),
                        Some(task.status),
                        Some(task.updated_at.clone()),
                        matches && task.status.is_writable(),
                        Some(matches),
                        reason.to_string(),
                    )
                }
                None => (
                    None,
                    None,
                    None,
                    true,
                    None,
                    "当前没有活动任务，工作区采用无任务模式；修改不会进入任务事件流".to_string(),
                ),
            };

        let mut capabilities = HashMap::new();
        capabilities.insert(
            "read".into(),
            CapabilityStatus {
                status: "available".into(),
                reason: "工作区读取不依赖活动任务".into(),
                recoverable: true,
            },
        );
        capabilities.insert(
            "write".into(),
            CapabilityStatus {
                status: if writable { "available" } else { "denied" }.into(),
                reason: if writable {
                    if task_id.is_some() {
                        "活动任务和工作区基线有效"
                    } else {
                        "无任务模式允许直接修改，建议需要长期追踪时调用 start_task"
                    }
                } else {
                    "需要活动任务且工作区基线必须匹配"
                }
                .into(),
                recoverable: true,
            },
        );
        capabilities.insert(
            "exec".into(),
            CapabilityStatus {
                status: if writable { "available" } else { "denied" }.into(),
                reason: if writable {
                    if task_id.is_some() {
                        "活动任务和工作区基线有效"
                    } else {
                        "无任务模式允许直接执行，建议需要长期追踪时调用 start_task"
                    }
                } else {
                    "需要活动任务且工作区基线必须匹配"
                }
                .into(),
                recoverable: true,
            },
        );
        capabilities.insert(
            "git".into(),
            CapabilityStatus {
                status: if branch.is_some() && head.is_some() {
                    "available"
                } else {
                    "degraded"
                }
                .into(),
                reason: if branch.is_some() && head.is_some() {
                    "已读取当前分支和 HEAD"
                } else {
                    "当前工作区不是可读取 Git 状态的仓库"
                }
                .into(),
                recoverable: true,
            },
        );
        capabilities.insert(
            "network".into(),
            CapabilityStatus {
                status: "managed_by_policy".into(),
                reason: "网络权限由工具策略控制，不由 Harness 任务状态决定".into(),
                recoverable: true,
            },
        );

        let mut next_actions = Vec::new();
        if task_id.is_none() {
            next_actions.push("start_task".into());
        } else if baseline_matches == Some(false) {
            next_actions.push("project_state".into());
            next_actions.push("git_diff".into());
            next_actions.push("refresh_baseline".into());
        } else if !writable {
            next_actions.push("resume_task".into());
        }
        next_actions.push("read_file".into());
        next_actions.push("git_status".into());

        Ok(HarnessStatus {
            schema_version: SCHEMA_VERSION,
            workspace_id: self.workspace_id.clone(),
            scope_id,
            scope_root: scope_root.to_string_lossy().to_string(),
            task_id,
            task_state,
            task_updated_at,
            writable,
            reason,
            recoverable: true,
            branch,
            head,
            current_head,
            task_baseline_head,
            baseline_matches,
            capabilities,
            next_actions,
        })
    }

    fn build_workspace_state(
        &self,
        existing: Option<WorkspaceHarnessState>,
    ) -> HarnessResult<WorkspaceHarnessState> {
        let tasks = self.store.list_tasks(&self.workspace_id)?;
        let mut active_task_ids = HashMap::new();
        for task in &tasks {
            if task.status.is_writable() {
                active_task_ids
                    .entry(self.task_scope_id(task).to_string())
                    .or_insert_with(|| task.id.clone());
            }
        }
        let previous_active = existing
            .as_ref()
            .and_then(|state| state.active_task_id.clone());
        let active_task_id = previous_active
            .filter(|id| active_task_ids.values().any(|active| active == id))
            .or_else(|| active_task_ids.get(&self.workspace_id).cloned())
            .or_else(|| active_task_ids.values().next().cloned());
        Ok(WorkspaceHarnessState {
            schema_version: SCHEMA_VERSION,
            active_task_id,
            active_task_ids: Some(active_task_ids),
            recent_task_ids: tasks.into_iter().take(20).map(|task| task.id).collect(),
            updated_at: existing
                .map(|state| state.updated_at)
                .filter(|value| !value.is_empty())
                .unwrap_or_else(timestamp),
        })
    }

    fn repair_workspace_state(
        &self,
        existing: Option<WorkspaceHarnessState>,
    ) -> HarnessResult<WorkspaceHarnessState> {
        let state = self.build_workspace_state(existing)?;
        self.store
            .save_workspace_state(&self.workspace_id, &state)?;
        Ok(state)
    }

    fn save_workspace_state(
        &self,
        active_task_id: Option<&str>,
        task: &TaskSession,
    ) -> HarnessResult<()> {
        let existing = self.store.load_workspace_state(&self.workspace_id)?;
        let mut state = match existing {
            Some(state) if state.active_task_ids.is_some() => state,
            state => self.build_workspace_state(state)?,
        };
        let active = state.active_task_ids.get_or_insert_with(HashMap::new);
        let scope_id = self.task_scope_id(task).to_string();
        if task.status.is_writable() {
            active.insert(scope_id, task.id.clone());
        } else if active.get(&scope_id).is_some_and(|id| id == &task.id) {
            active.remove(&scope_id);
        }
        state.schema_version = SCHEMA_VERSION;
        state.active_task_id = active_task_id.map(str::to_string);
        state.recent_task_ids.retain(|id| id != &task.id);
        state.recent_task_ids.insert(0, task.id.clone());
        state.recent_task_ids.truncate(20);
        state.updated_at = task.updated_at.clone();
        self.store.save_workspace_state(&self.workspace_id, &state)
    }
}

fn git_identity(root: &Path) -> (Option<String>, Option<String>) {
    if let Some((branch, head)) = fast_workspace_git_identity(root) {
        return (Some(branch), Some(head));
    }
    std::thread::scope(|scope| {
        let branch = scope.spawn(|| git_value(root, &["rev-parse", "--abbrev-ref", "HEAD"]));
        let head = scope.spawn(|| git_value(root, &["rev-parse", "HEAD"]));
        (branch.join().unwrap_or(None), head.join().unwrap_or(None))
    })
}

pub fn capture_baseline(root: &Path) -> ProjectBaseline {
    let paths = git_baseline_paths(root).unwrap_or_else(|| {
        WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
            .filter_map(|item| {
                let path = item.path();
                if path == root || should_skip(path, root) || !item.file_type().is_file() {
                    return None;
                }
                Some(
                    path.strip_prefix(root)
                        .unwrap_or(path)
                        .to_string_lossy()
                        .replace('\\', "/"),
                )
            })
            .collect::<Vec<_>>()
    });
    let (mut entries, (branch, head)) = std::thread::scope(|scope| {
        let entries = scope.spawn(|| baseline_entries_for_paths(root, paths));
        let identity = scope.spawn(|| git_identity(root));
        (
            entries.join().unwrap_or_default(),
            identity.join().unwrap_or((None, None)),
        )
    });
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let mut fingerprint = Sha256::new();
    for entry in &entries {
        fingerprint.update(entry.path.as_bytes());
        fingerprint.update(entry.sha256.as_bytes());
        fingerprint.update(entry.bytes.to_le_bytes());
    }
    ProjectBaseline {
        branch,
        head,
        worktree_fingerprint: format!("{:x}", fingerprint.finalize()),
        entries,
        captured_at: timestamp(),
    }
}

fn git_baseline_paths(root: &Path) -> Option<Vec<String>> {
    let output = git_output(
        root,
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
    )?;
    let mut paths = output
        .split(|byte| *byte == 0)
        .filter(|value| !value.is_empty())
        .map(|value| String::from_utf8_lossy(value).replace('\\', "/"))
        .filter(|value| !value.ends_with('/'))
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    Some(paths)
}

fn baseline_entries_for_paths(root: &Path, paths: Vec<String>) -> Vec<BaselineEntry> {
    if paths.is_empty() {
        return Vec::new();
    }
    let workers = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(4)
        .clamp(1, 16)
        .min(paths.len());
    let chunk_size = (paths.len() + workers - 1) / workers;
    std::thread::scope(|scope| {
        let handles = paths
            .chunks(chunk_size)
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .filter_map(|relative| baseline_entry(root, relative))
                        .collect::<Vec<_>>()
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap_or_default())
            .collect()
    })
}

fn baseline_entry(root: &Path, relative: &str) -> Option<BaselineEntry> {
    let full = root.join(relative);
    let metadata = fs::symlink_metadata(&full).ok()?;
    let (bytes, is_binary) = if metadata.file_type().is_symlink() {
        (
            fs::read_link(&full)
                .ok()?
                .to_string_lossy()
                .replace('\\', "/")
                .into_bytes(),
            false,
        )
    } else if metadata.is_file() {
        let bytes = fs::read(&full).ok()?;
        let is_binary = bytes.contains(&0);
        (bytes, is_binary)
    } else {
        return None;
    };
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Some(BaselineEntry {
        path: relative.replace('\\', "/"),
        exists: true,
        is_binary,
        sha256: format!("{:x}", hasher.finalize()),
        bytes: bytes.len() as u64,
    })
}

fn change_records(baseline: &ProjectBaseline, current: &ProjectBaseline) -> Vec<FileChangeRecord> {
    let baseline_map = baseline
        .entries
        .iter()
        .map(|entry| (entry.path.clone(), entry))
        .collect::<HashMap<_, _>>();
    let current_map = current
        .entries
        .iter()
        .map(|entry| (entry.path.clone(), entry))
        .collect::<HashMap<_, _>>();
    let mut paths = baseline_map
        .keys()
        .chain(current_map.keys())
        .cloned()
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter_map(|path| {
            let before = baseline_map.get(&path);
            let after = current_map.get(&path);
            let status = match (before, after) {
                (Some(before), Some(after)) if before.sha256 == after.sha256 => "unchanged",
                (Some(_), Some(_)) => "modified",
                (Some(_), None) => "deleted",
                (None, Some(_)) => "added",
                (None, None) => "unknown",
            };
            (status != "unchanged").then(|| FileChangeRecord {
                path,
                status: status.to_string(),
                before_sha256: before.map(|entry| entry.sha256.clone()),
                after_sha256: after.map(|entry| entry.sha256.clone()),
            })
        })
        .collect()
}

fn is_change_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn should_skip(path: &Path, root: &Path) -> bool {
    path.strip_prefix(root)
        .ok()
        .into_iter()
        .flat_map(|p| p.components())
        .filter_map(|component| component.as_os_str().to_str())
        .any(|name| {
            matches!(
                name,
                ".git"
                    | ".mcp-probe-kit"
                    | "node_modules"
                    | "target"
                    | "dist"
                    | "build"
                    | ".svelte-kit"
            )
        })
}

fn git_value(root: &Path, args: &[&str]) -> Option<String> {
    let output = git_output(root, args)?;
    let value = String::from_utf8_lossy(&output).trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn git_output(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let args = args
        .iter()
        .map(|arg| (*arg).to_string())
        .collect::<Vec<_>>();
    let git_env = [("GIT_TERMINAL_PROMPT".to_string(), "0".to_string())];
    let mut command =
        crate::platform::wsl::std_command_for_workspace_with_env("git", &args, root, &git_env, &[]);

    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(output.stdout)
}

fn workspace_id(root: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(root.to_string_lossy().as_bytes());
    format!("{:x}", hasher.finalize())[..32].to_string()
}

fn timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().to_string())
        .unwrap_or_else(|_| "0".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::tempdir;

    fn git(root: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(root)
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?}");
    }

    fn init_git_workspace(root: &Path) {
        git(root, &["init"]);
        git(root, &["config", "user.name", "Harness Test"]);
        git(root, &["config", "user.email", "harness@example.invalid"]);
        fs::write(root.join(".gitignore"), "runtime-data/\ncache/\n").expect("gitignore");
        fs::write(root.join("tracked.txt"), "initial\n").expect("tracked");
        git(root, &["add", "--all"]);
        git(root, &["commit", "-m", "initial"]);
    }

    #[test]
    fn status_keeps_read_available_without_task() {
        let workspace = tempdir().expect("workspace");
        let harness_root = tempdir().expect("harness");
        fs::write(workspace.path().join("main.rs"), "fn main() {}\n").expect("file");
        let harness = Harness::new(
            workspace.path().to_path_buf(),
            harness_root.path().to_path_buf(),
        )
        .expect("harness");

        let status = harness.status().expect("status");
        assert!(status.writable);
        assert_eq!(status.capabilities["read"].status, "available");
        assert_eq!(status.capabilities["write"].status, "available");
        assert!(status.next_actions.contains(&"start_task".to_string()));
    }

    #[test]
    fn status_reports_git_identity_without_active_task() {
        let workspace = tempdir().expect("workspace");
        let harness_root = tempdir().expect("harness");
        init_git_workspace(workspace.path());
        let harness = Harness::new(
            workspace.path().to_path_buf(),
            harness_root.path().to_path_buf(),
        )
        .expect("harness");

        let status = harness.status().expect("status");
        assert!(status.branch.is_some());
        assert!(status.head.is_some());
        assert_eq!(status.capabilities["git"].status, "available");
    }

    #[test]
    fn starting_task_does_not_create_workspace_copies() {
        let workspace = tempdir().expect("workspace");
        let harness_root = tempdir().expect("harness");
        fs::write(workspace.path().join("main.rs"), "fn main() {}\n").expect("file");
        let harness = Harness::new(
            workspace.path().to_path_buf(),
            harness_root.path().to_path_buf(),
        )
        .expect("harness");

        harness.start_task("测试任务").expect("start task");
        assert!(!harness
            .store_root()
            .join("workspaces")
            .join(harness.workspace_id())
            .join("snapshots")
            .exists());
    }

    #[test]
    fn terminal_task_is_removed_from_active_task_index() {
        let workspace = tempdir().expect("workspace");
        let harness_root = tempdir().expect("harness");
        let harness = Harness::new(
            workspace.path().to_path_buf(),
            harness_root.path().to_path_buf(),
        )
        .expect("harness");
        let task = harness.start_task("terminal index removal").expect("task");
        harness
            .transition(&task.id, TaskStatus::CompletedUnverified)
            .expect("complete task");
        assert!(harness.current_task().expect("current task").is_none());
        let state = harness
            .store
            .load_workspace_state(harness.workspace_id())
            .expect("state")
            .expect("workspace state");
        assert!(state
            .active_task_ids
            .expect("active task index")
            .get(harness.workspace_id())
            .is_none());

        let reopened = Harness::new(
            workspace.path().to_path_buf(),
            harness_root.path().to_path_buf(),
        )
        .expect("reopened harness");
        assert!(reopened.current_task().expect("reopened current").is_none());
    }

    #[test]
    fn legacy_workspace_state_repairs_active_task_index_on_read() {
        let workspace = tempdir().expect("workspace");
        let harness_root = tempdir().expect("harness");
        let harness = Harness::new(
            workspace.path().to_path_buf(),
            harness_root.path().to_path_buf(),
        )
        .expect("harness");
        let task = harness.start_task("legacy index migration").expect("task");
        let state_path = harness
            .store_root()
            .join("workspaces")
            .join(harness.workspace_id())
            .join("state.json");
        let mut legacy: serde_json::Value =
            serde_json::from_slice(&fs::read(&state_path).expect("state bytes"))
                .expect("state json");
        legacy
            .as_object_mut()
            .expect("state object")
            .remove("active_task_ids");
        fs::write(
            &state_path,
            serde_json::to_vec_pretty(&legacy).expect("legacy state json"),
        )
        .expect("legacy state write");

        let reopened = Harness::new(
            workspace.path().to_path_buf(),
            harness_root.path().to_path_buf(),
        )
        .expect("reopened harness");
        assert_eq!(
            reopened
                .current_task()
                .expect("current task")
                .expect("active task")
                .id,
            task.id
        );
        let repaired: WorkspaceHarnessState =
            serde_json::from_slice(&fs::read(&state_path).expect("repaired state bytes"))
                .expect("repaired state json");
        assert_eq!(
            repaired
                .active_task_ids
                .expect("repaired active task index")
                .get(reopened.workspace_id()),
            Some(&task.id)
        );
    }

    #[test]
    fn active_tasks_are_scoped_by_linked_worktree_and_persist() {
        let workspace = tempdir().expect("workspace");
        let harness_root = tempdir().expect("harness");
        init_git_workspace(workspace.path());
        let linked = workspace.path().join(".worktrees").join("scoped-task");
        fs::create_dir_all(linked.parent().expect("worktree parent")).expect("worktree parent");
        let linked_text = linked.to_string_lossy().to_string();
        git(
            workspace.path(),
            &[
                "worktree",
                "add",
                "-b",
                "task-scope-test",
                linked_text.as_str(),
            ],
        );

        let harness = Harness::new(
            workspace.path().to_path_buf(),
            harness_root.path().to_path_buf(),
        )
        .expect("harness");
        let root_scope = harness.scope_root_for(workspace.path());
        let linked_scope = harness.scope_root_for(&linked);
        let root_task = harness
            .start_task_for("root task", &root_scope)
            .expect("root task");
        let linked_task = harness
            .start_task_for("linked task", &linked_scope)
            .expect("linked task");
        let indexed_state = harness
            .store
            .load_workspace_state(harness.workspace_id())
            .expect("indexed state")
            .expect("workspace state");
        let active = indexed_state.active_task_ids.expect("active task index");
        assert_eq!(
            active.get(&root_task.scope_id.clone().unwrap()),
            Some(&root_task.id)
        );
        assert_eq!(
            active.get(&linked_task.scope_id.clone().unwrap()),
            Some(&linked_task.id)
        );

        assert_eq!(root_task.workspace_id, linked_task.workspace_id);
        assert_ne!(root_task.scope_id, linked_task.scope_id);
        assert_eq!(
            harness
                .current_task_for_root(&root_scope)
                .expect("root current")
                .unwrap()
                .id,
            root_task.id
        );
        assert_eq!(
            harness
                .current_task_for_root(&linked_scope)
                .expect("linked current")
                .unwrap()
                .id,
            linked_task.id
        );
        assert_eq!(
            harness
                .status_for(&root_scope)
                .expect("root status")
                .task_id,
            Some(root_task.id.clone())
        );
        assert_eq!(
            harness
                .status_for(&linked_scope)
                .expect("linked status")
                .task_id,
            Some(linked_task.id.clone())
        );

        let reopened = Harness::new(
            workspace.path().to_path_buf(),
            harness_root.path().to_path_buf(),
        )
        .expect("reopen harness");
        assert_eq!(
            reopened
                .current_task_for_root(&root_scope)
                .expect("restored root")
                .unwrap()
                .id,
            root_task.id
        );
        assert_eq!(
            reopened
                .current_task_for_root(&linked_scope)
                .expect("restored linked")
                .unwrap()
                .id,
            linked_task.id
        );
    }

    #[test]
    fn git_baseline_respects_ignores_and_nested_worktree_boundaries() {
        let workspace = tempdir().expect("workspace");
        init_git_workspace(workspace.path());
        fs::create_dir_all(workspace.path().join("runtime-data")).expect("runtime data");
        fs::write(workspace.path().join("runtime-data/state.json"), "one\n").expect("state");
        fs::write(workspace.path().join("notes.txt"), "before\n").expect("notes");
        let linked = workspace.path().join("linked-worktree");
        let linked_text = linked.to_string_lossy().to_string();
        git(
            workspace.path(),
            &["worktree", "add", "-b", "linked-test", linked_text.as_str()],
        );

        let baseline = capture_baseline(workspace.path());
        assert!(baseline
            .entries
            .iter()
            .any(|entry| entry.path == "notes.txt"));
        assert!(!baseline
            .entries
            .iter()
            .any(|entry| entry.path.starts_with("runtime-data/")));
        assert!(!baseline
            .entries
            .iter()
            .any(|entry| entry.path.starts_with("linked-worktree/")));

        fs::write(workspace.path().join("runtime-data/state.json"), "two\n").expect("state");
        fs::write(linked.join("tracked.txt"), "linked change\n").expect("linked change");
        assert_eq!(
            capture_baseline(workspace.path()).worktree_fingerprint,
            baseline.worktree_fingerprint
        );

        fs::write(workspace.path().join("notes.txt"), "after\n").expect("notes");
        assert_ne!(
            capture_baseline(workspace.path()).worktree_fingerprint,
            baseline.worktree_fingerprint
        );
    }
}
