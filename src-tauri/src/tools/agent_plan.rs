//! Agent-reported plan tools: `set_todos`, `update_plan` and `report_progress`.
//!
//! They let the AI publish its goal, a step checklist and free-form progress so
//! the desktop task panel can show "what is being built, which step is running
//! and how far along it is". State lives in the in-memory activity feed of the
//! workspace profile; nothing touches the workspace files.

use serde_json::{json, Value};

use crate::mcp::activity::{
    self, ActivityPlan, ActivityProgress, ActivityTodo, MAX_EXTERNAL_ID_CHARS, MAX_GOAL_CHARS,
    MAX_PHASE_CHARS, MAX_PROGRESS_CHARS, MAX_TODOS, MAX_TODO_TITLE_CHARS,
};
use crate::tools::context::ToolContext;
use crate::tools::workspace::{tool_ok, WorkspaceError};

fn invalid(message: impl Into<String>) -> WorkspaceError {
    WorkspaceError::invalid_argument(message)
}

/// Optional bounded text argument. Absent or null → `None`.
fn optional_text(args: &Value, key: &str, max: usize) -> Result<Option<String>, WorkspaceError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => {
            if text.chars().count() > max {
                Err(invalid(format!("{key} must be at most {max} characters")))
            } else {
                Ok(Some(text.trim().to_string()))
            }
        }
        Some(_) => Err(invalid(format!("{key} must be a string"))),
    }
}

fn required_text(args: &Value, key: &str, max: usize) -> Result<String, WorkspaceError> {
    optional_text(args, key, max)?
        .filter(|text| !text.is_empty())
        .ok_or_else(|| invalid(format!("{key} is required")))
}

fn item_text(item: &Value, key: &str, index: usize) -> Result<String, WorkspaceError> {
    item.get(key)
        .and_then(Value::as_str)
        .map(|text| text.trim().to_string())
        .ok_or_else(|| invalid(format!("item {index}: {key} must be a string")))
}

fn items<'a>(args: &'a Value, key: &str) -> Result<&'a [Value], WorkspaceError> {
    let list = args
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| invalid(format!("{key} must be an array")))?;
    if list.len() > MAX_TODOS {
        return Err(invalid(format!(
            "{key} must contain at most {MAX_TODOS} items"
        )));
    }
    Ok(list.as_slice())
}

fn plan_summary(plan: Option<&ActivityPlan>) -> Value {
    let Some(plan) = plan else {
        return json!({ "cleared": true, "todos": [] });
    };
    let done = plan
        .todos
        .iter()
        .filter(|todo| todo.status == "completed")
        .count();
    let current = plan
        .todos
        .iter()
        .find(|todo| todo.status == "in_progress")
        .map(|todo| json!({ "id": todo.id, "title": todo.title }));
    json!({
        "goal": plan.objective,
        "status": plan.status,
        "completed": done,
        "total": plan.todos.len(),
        "current": current,
        "todos": plan.todos,
        "external_task_id": plan.external_task_id,
        "progress": plan.progress,
    })
}

pub fn set_todos(ctx: &ToolContext, args: &Value) -> Result<Value, WorkspaceError> {
    let goal = optional_text(args, "goal", MAX_GOAL_CHARS)?;
    let external_task_id =
        optional_text(args, "external_task_id", MAX_EXTERNAL_ID_CHARS)?.filter(|id| !id.is_empty());
    let todos = items(args, "todos")?
        .iter()
        .enumerate()
        .map(|(index, item)| {
            Ok(ActivityTodo {
                id: item_text(item, "id", index)?,
                title: item_text(item, "title", index)?,
                status: item_text(item, "status", index)?,
            })
        })
        .collect::<Result<Vec<_>, WorkspaceError>>()?;
    let plan = activity::set_agent_todos(&ctx.profile_id, goal, external_task_id, todos)
        .map_err(invalid)?;
    Ok(tool_ok(json!({ "plan": plan_summary(plan.as_ref()) })))
}

pub fn update_plan(ctx: &ToolContext, args: &Value) -> Result<Value, WorkspaceError> {
    let goal = optional_text(args, "goal", MAX_GOAL_CHARS)?;
    let explanation = optional_text(args, "explanation", MAX_PROGRESS_CHARS)?;
    let steps = items(args, "plan")?
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let step = item_text(item, "step", index)?;
            if step.is_empty() || step.chars().count() > MAX_TODO_TITLE_CHARS {
                return Err(invalid(format!(
                    "item {index}: step must be 1-{MAX_TODO_TITLE_CHARS} characters"
                )));
            }
            Ok((step, item_text(item, "status", index)?))
        })
        .collect::<Result<Vec<_>, WorkspaceError>>()?;
    let plan =
        activity::update_agent_plan(&ctx.profile_id, goal, steps, explanation).map_err(invalid)?;
    Ok(tool_ok(json!({ "plan": plan_summary(plan.as_ref()) })))
}

pub fn report_progress(ctx: &ToolContext, args: &Value) -> Result<Value, WorkspaceError> {
    let message = required_text(args, "message", MAX_PROGRESS_CHARS)?;
    let phase = optional_text(args, "phase", MAX_PHASE_CHARS)?.unwrap_or_default();
    let percent = match args.get("percent") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_u64()
                .filter(|percent| *percent <= 100)
                .map(|percent| percent as u8)
                .ok_or_else(|| invalid("percent must be an integer from 0 to 100"))?,
        ),
    };
    let todo_id =
        optional_text(args, "todo_id", activity::MAX_TODO_ID_CHARS)?.filter(|id| !id.is_empty());
    let progress: ActivityProgress =
        activity::report_agent_progress(&ctx.profile_id, message, phase, percent, todo_id)
            .map_err(invalid)?;
    Ok(tool_ok(json!({ "progress": progress })))
}
