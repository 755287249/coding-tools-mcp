import { readOnlyTools, toolNames } from './catalog.js';
import type { JsonObject } from './types.js';

export type ToolDomain =
  | 'harness'
  | 'runtime'
  | 'history'
  | 'task'
  | 'filesystem'
  | 'search'
  | 'quality'
  | 'process'
  | 'git'
  | 'desktop';

export type ToolExecutionLane = 'blocking' | 'process' | 'control';
export type ToolAdmissionMode = 'request' | 'children' | 'none';
export type ToolLockGroup = 'history' | 'workspace_content' | 'git' | 'task' | 'cwd';
export type ToolPermissionKind = 'workspace_mutation' | 'process_execution' | 'network' | 'privileged_operation';
export type ToolUsageFamily = 'filesystem' | 'search' | 'quality' | 'process' | 'git' | 'history' | 'runtime' | 'other';
export type ToolCoalescingPolicy = 'never' | 'read_only' | 'operation_id';
export type ToolMutationPolicy = 'never' | 'always' | 'format_apply';

export interface ToolRuntimeDescriptor {
  readonly name: string;
  readonly canonicalName: string;
  readonly domain: ToolDomain;
  readonly usageFamily: ToolUsageFamily;
  readonly lane: ToolExecutionLane;
  readonly admission: ToolAdmissionMode;
  readonly lockGroups: readonly ToolLockGroup[];
  readonly harnessTool: boolean;
  readonly guardedPermission?: ToolPermissionKind;
  readonly coalescing: ToolCoalescingPolicy;
  readonly mutation: ToolMutationPolicy;
  readonly workspaceSelector: boolean;
}

type ToolRuntimeOverrides = Partial<Pick<ToolRuntimeDescriptor,
  'usageFamily' | 'lane' | 'admission' | 'lockGroups' | 'harnessTool' | 'guardedPermission' | 'coalescing' | 'mutation' | 'workspaceSelector'
>>;

interface ToolRuntimeModule {
  readonly domain: ToolDomain;
  readonly usageFamily: ToolUsageFamily;
  readonly defaults?: ToolRuntimeOverrides;
  readonly tools: Readonly<Record<string, ToolRuntimeOverrides>>;
}

const WORKSPACE_CONTENT_LOCK = ['workspace_content'] as const;
const GIT_LOCK = ['git'] as const;
const TASK_LOCK = ['task'] as const;

function workspaceSelectorFor(name: string): boolean {
  return name.startsWith('git_') || [
    'set_default_cwd',
    'read_file', 'read_many', 'list_files', 'project_map', 'search_text',
    'apply_patch', 'edit', 'file_ops', 'patch_check', 'format_files', 'view_image',
    'exec_health_check', 'exec_command', 'exec_many', 'wait_command', 'resolve_operation',
    'list_sessions', 'send_input', 'kill_session', 'read_output', 'request_permissions'
  ].includes(name);
}

const TOOL_RUNTIME_MODULES: readonly ToolRuntimeModule[] = [
  {
    domain: 'harness',
    usageFamily: 'other',
    defaults: { harnessTool: true },
    tools: {
      harness_status: {},
      operation_log: {}
    }
  },
  {
    domain: 'runtime',
    usageFamily: 'other',
    tools: {
      server_info: { usageFamily: 'runtime' },
      list_workspace_folders: {},
      conversation_bootstrap: { lockGroups: ['history'], coalescing: 'never' },
      switch_workspace_folder: { coalescing: 'never' },
      query_tool_usage: { usageFamily: 'runtime' },
      set_default_cwd: { usageFamily: 'runtime', lockGroups: ['cwd'] },
      request_permissions: { usageFamily: 'runtime', lane: 'control', coalescing: 'never' }
    }
  },
  {
    domain: 'history',
    usageFamily: 'history',
    defaults: { lockGroups: ['history'] },
    tools: {
      history_session_bootstrap: {},
      history_session_checkpoint: {},
      history_session_validate: {}
    }
  },
  {
    domain: 'task',
    usageFamily: 'other',
    defaults: { harnessTool: true },
    tools: {
      project_state: {},
      start_task: { lockGroups: TASK_LOCK, mutation: 'always' },
      update_task: { lockGroups: TASK_LOCK, mutation: 'always' },
      pause_task: { lockGroups: TASK_LOCK, mutation: 'always' },
      resume_task: { lockGroups: TASK_LOCK, mutation: 'always' },
      fail_task: { lockGroups: TASK_LOCK, mutation: 'always' },
      close_failed_task: { lockGroups: TASK_LOCK, mutation: 'always' },
      rollback_task: { lockGroups: TASK_LOCK, mutation: 'always' },
      finish_task: { lockGroups: TASK_LOCK, mutation: 'always' },
      task_context: {},
      list_task_events: {},
      change_summary: {}
    }
  },
  {
    domain: 'filesystem',
    usageFamily: 'filesystem',
    tools: {
      read_file: {},
      read_many: {},
      list_files: {},
      apply_patch: {
        lockGroups: WORKSPACE_CONTENT_LOCK,
        guardedPermission: 'workspace_mutation',
        mutation: 'always'
      },
      edit: {
        lockGroups: WORKSPACE_CONTENT_LOCK,
        guardedPermission: 'workspace_mutation',
        mutation: 'always'
      },
      file_ops: {
        lockGroups: WORKSPACE_CONTENT_LOCK,
        guardedPermission: 'workspace_mutation',
        mutation: 'always'
      },
      patch_check: { usageFamily: 'other' },
      view_image: {}
    }
  },
  {
    domain: 'desktop',
    usageFamily: 'runtime',
    tools: {
      desktop_displays: {},
      desktop_screenshot: {},
      desktop_click: { mutation: 'always' },
      desktop_drag: { mutation: 'always' },
      desktop_scroll: { mutation: 'always' },
      desktop_type: { mutation: 'always' },
      desktop_key: { mutation: 'always' }
    }
  },
  {
    domain: 'search',
    usageFamily: 'search',
    tools: {
      project_map: {},
      search_text: {}
    }
  },
  {
    domain: 'quality',
    usageFamily: 'quality',
    tools: {
      format_files: {
        lockGroups: WORKSPACE_CONTENT_LOCK,
        guardedPermission: 'privileged_operation',
        mutation: 'format_apply'
      }
    }
  },
  {
    domain: 'process',
    usageFamily: 'process',
    tools: {
      exec_health_check: { usageFamily: 'other' },
      exec_command: {
        lane: 'process',
        guardedPermission: 'process_execution',
        coalescing: 'operation_id',
        mutation: 'always'
      },
      exec_many: {
        lane: 'process',
        admission: 'children',
        guardedPermission: 'process_execution',
        mutation: 'always'
      },
      wait_command: { lane: 'control' },
      resolve_operation: { lane: 'control' },
      list_sessions: { lane: 'control' },
      send_input: { lane: 'control' },
      kill_session: {
        lane: 'control',
        guardedPermission: 'process_execution',
        mutation: 'always'
      },
      read_output: { lane: 'control' }
    }
  },
  {
    domain: 'git',
    usageFamily: 'git',
    tools: {
      git_status: {},
      git_diff: {},
      git_log: {},
      git_show: {},
      git_blame: {},
      git_branch: {
        lockGroups: GIT_LOCK,
        guardedPermission: 'workspace_mutation',
        mutation: 'always'
      },
      git_worktree: {
        lockGroups: ['git', 'workspace_content'],
        guardedPermission: 'workspace_mutation',
        mutation: 'always'
      },
      git_stage: {
        lockGroups: GIT_LOCK,
        guardedPermission: 'workspace_mutation',
        mutation: 'always'
      },
      git_commit: {
        lockGroups: GIT_LOCK,
        guardedPermission: 'workspace_mutation',
        mutation: 'always'
      },
      git_push: { guardedPermission: 'network' },
      git_restore: {
        lockGroups: ['git', 'workspace_content'],
        guardedPermission: 'workspace_mutation',
        mutation: 'always'
      }
    }
  }
];

const runtimeByName = new Map<string, ToolRuntimeDescriptor>();

for (const moduleDefinition of TOOL_RUNTIME_MODULES) {
  for (const [name, overrides] of Object.entries(moduleDefinition.tools)) {
    if (runtimeByName.has(name)) throw new Error(`Duplicate Node tool runtime metadata: ${name}`);
    const configured = { ...moduleDefinition.defaults, ...overrides };
    const lane = configured.lane ?? 'blocking';
    runtimeByName.set(name, Object.freeze({
      name,
      canonicalName: name,
      domain: moduleDefinition.domain,
      usageFamily: configured.usageFamily ?? moduleDefinition.usageFamily,
      lane,
      admission: configured.admission ?? (lane === 'control' ? 'none' : 'request'),
      lockGroups: Object.freeze([...(configured.lockGroups ?? [])]),
      harnessTool: configured.harnessTool ?? false,
      guardedPermission: configured.guardedPermission,
      coalescing: configured.coalescing ?? (readOnlyTools.has(name) ? 'read_only' : 'never'),
      mutation: configured.mutation ?? 'never',
      workspaceSelector: configured.workspaceSelector ?? workspaceSelectorFor(name)
    }));
  }
}

const catalogNames = new Set(toolNames);
const missingRuntime = toolNames.filter(name => !runtimeByName.has(name));
const unknownRuntime = [...runtimeByName.keys()].filter(name => !catalogNames.has(name));
if (missingRuntime.length || unknownRuntime.length) {
  throw new Error([
    missingRuntime.length ? `Missing Node tool runtime metadata: ${missingRuntime.join(', ')}` : '',
    unknownRuntime.length ? `Unknown Node tool runtime metadata: ${unknownRuntime.join(', ')}` : ''
  ].filter(Boolean).join('; '));
}

for (const alias of ['edit_file', 'edit_many']) {
  const edit = runtimeByName.get('edit');
  if (!edit) throw new Error('Node tool runtime alias target is missing: edit');
  runtimeByName.set(alias, Object.freeze({ ...edit, name: alias, canonicalName: 'edit' }));
}

const unknownRuntimeDefaults = Object.freeze({
  domain: 'runtime' as const,
  usageFamily: 'other' as const,
  lane: 'blocking' as const,
  admission: 'request' as const,
  lockGroups: Object.freeze([]) as readonly ToolLockGroup[],
  harnessTool: false,
  coalescing: 'never' as const,
  mutation: 'never' as const,
  workspaceSelector: false
});

export function registeredToolRuntimeNames(): string[] {
  return toolNames.filter(name => runtimeByName.has(name));
}

export function toolRuntimeFor(name: string): ToolRuntimeDescriptor {
  return runtimeByName.get(name) ?? {
    ...unknownRuntimeDefaults,
    name,
    canonicalName: name
  };
}

export function canCoalesceToolCall(name: string, args: JsonObject): boolean {
  const policy = toolRuntimeFor(name).coalescing;
  if (policy === 'read_only') return true;
  return policy === 'operation_id' && String(args.operation_id ?? '').trim().length > 0;
}

export function canonicalToolCall(name: string, args: JsonObject): { name: string; args: JsonObject } {
  const canonicalName = toolRuntimeFor(name).canonicalName;
  if (name === 'edit_file') {
    const file: JsonObject = {};
    for (const field of ['path', 'expected_sha256', 'edits', 'apply_proposal']) {
      if (args[field] !== undefined) file[field] = args[field];
    }
    const converted: JsonObject = { files: [file] };
    if (args.dry_run !== undefined) converted.dry_run = args.dry_run;
    if (args.reason !== undefined) converted.reason = args.reason;
    return { name: canonicalName, args: converted };
  }
  return { name: canonicalName, args };
}

export function requestMutates(tool: string, args: JsonObject): boolean {
  const policy = toolRuntimeFor(tool).mutation;
  if (policy === 'always') return true;
  if (policy === 'format_apply') return String(args.mode ?? '') === 'apply';
  return false;
}

export function toolUsageFamily(tool: string): ToolUsageFamily {
  return toolRuntimeFor(tool).usageFamily;
}
