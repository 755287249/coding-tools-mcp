mod agent_plan;
pub mod context;
mod desktop;
pub mod dispatch;
pub mod exec;
pub(crate) mod execution_timeout;
pub mod file;
pub mod file_action;
pub mod git;
pub mod history;
pub mod hub;
mod image_tool;
pub(crate) mod parallel_stats;
pub mod patch;
pub mod permission;
pub mod policy;
mod process_child;
mod process_spec;
mod process_start;
pub mod project;
pub mod redaction;
pub mod registry;
mod registry_metadata;
mod registry_schemas;
pub mod sandbox;
pub mod session;
pub(crate) mod tool_runtime;
pub mod tool_usage;
pub mod workspace;

pub use context::{
    ExecutionLimits, RuntimeToolConfig, SharedRuntimeToolConfig, SharedToolContext, ToolContext,
    ABSOLUTE_COMMAND_TIMEOUT_MAX_MS, DEFAULT_COMMAND_TIMEOUT_MAX_MS,
};
pub(crate) use dispatch::call_tool_async_with_canary_sample_key;
/// 唯一工具执行入口；MCP 与 Actions 必须调用这些共享入口，不得分叉实现。
pub use dispatch::{call_tool, call_tool_async};
pub use policy::{validate_actions_exposure, PolicySettings};
pub use registry::{
    exposed_tool_names, is_allowed_tool, is_mcp_only_tool, list_tools, list_tools_for_profile,
};
pub(crate) use tool_runtime::is_mutating_tool;
pub use workspace::{wrap_mcp_tool_result, wrap_tool_result, Workspace};

pub(crate) use process_start::behavioral_parity_fixture as process_start_behavioral_parity_fixture;
