import { access, readFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptPath = fileURLToPath(import.meta.url);
const workspace = resolve(dirname(scriptPath), '..');

const files = {
  checklist: 'docs/todo/node-agent-ui-parity/CHECKLIST.md',
  package: 'packages/node-agent/package.json',
  management: 'packages/node-agent/src/management.ts',
  managementRouter: 'packages/node-agent/src/management/router.ts',
  managementObservabilityRoute: 'packages/node-agent/src/management/routes/observability.ts',
  observability: 'packages/node-agent/src/managementObservability.ts',
  taskTools: 'packages/node-agent/src/taskTools.ts',
  operationSummary: 'packages/node-agent/src/operationSummary.ts',
  processes: 'packages/node-agent/src/processes.ts',
  processHarnessTracking: 'packages/node-agent/src/processes/harnessTracking.ts',
  processDispatcher: 'packages/node-agent/src/toolDispatchers/process.ts',
  sandboxAppContainer: 'packages/node-agent/src/sandboxAppContainer.ts',
  appContainerHelperBuild: 'packages/node-agent/scripts/build-appcontainer-helper.mjs',
  rustDispatch: 'src-tauri/src/tools/dispatch.rs',
  rustDispatchTracking: 'src-tauri/src/tools/dispatch/tracking.rs',
  rustBuiltinTunnel: 'src-tauri/src/tunnel/builtin.rs',
  rustBuiltinConnection: 'src-tauri/src/tunnel/builtin/connection.rs',
  rustBuiltinMetrics: 'src-tauri/src/tunnel/builtin/metrics.rs',
  rustBuiltinPoolPolicy: 'src-tauri/src/tunnel/builtin/pool_policy.rs',
  rustBuiltinProtocolIo: 'src-tauri/src/tunnel/builtin/protocol_io.rs',
  rustBuiltinRequestMapping: 'src-tauri/src/tunnel/builtin/request_mapping.rs',
  rustBuiltinTests: 'src-tauri/src/tunnel/builtin/tests.rs',
  rustExec: 'src-tauri/src/tools/exec.rs',
  rustExecAdmission: 'src-tauri/src/tools/exec/admission.rs',
  rustExecBackend: 'src-tauri/src/tools/exec/backend.rs',
  rustExecIdentity: 'src-tauri/src/tools/exec/identity.rs',
  rustExecLifecycle: 'src-tauri/src/tools/exec/lifecycle.rs',
  rustExecNativeDiagnostic: 'src-tauri/src/tools/exec/native_diagnostic.rs',
  rustExecPostCheck: 'src-tauri/src/tools/exec/post_check.rs',
  rustExecRequest: 'src-tauri/src/tools/exec/request.rs',
  rustExecResult: 'src-tauri/src/tools/exec/result.rs',
  rustAppContainerProvider: 'src-tauri/src/tools/sandbox/appcontainer/provider.rs',
  rustProcessStart: 'src-tauri/src/tools/process_start.rs',
  rustSession: 'src-tauri/src/tools/session.rs',
  rustSessionAttachment: 'src-tauri/src/tools/session/attachment.rs',
  rustSessionConstruction: 'src-tauri/src/tools/session/construction.rs',
  rustSessionControl: 'src-tauri/src/tools/session/control.rs',
  rustSessionLifecycle: 'src-tauri/src/tools/session/lifecycle.rs',
  rustSessionProcessLifecycle: 'src-tauri/src/tools/session/process_lifecycle.rs',
  rustSessionRegistry: 'src-tauri/src/tools/session/registry.rs',
  workspacePage: 'src/routes/workspace/[id]/+page.svelte',
  telemetryViewer: 'src/lib/components/TelemetryViewer.svelte',
  operationLogViewer: 'src/lib/components/OperationLogViewer.svelte',
  historyViewer: 'src/lib/components/HistoryViewer.svelte',
  healthPanel: 'src/lib/components/HealthPanel.svelte',
  runtimePolicyForm: 'src/lib/components/RuntimePolicyForm.svelte',
  frontendCapabilities: 'src/lib/backend/capabilities.ts',
  nodeBackend: 'src/lib/backend/node.ts',
  managementUi: 'packages/node-agent/src/managementUi.ts',
  managementTest: 'packages/node-agent/test/management.test.mjs',
  harnessTest: 'packages/node-agent/test/harnessBaseline.test.mjs'
};

async function exists(path) {
  try { await access(path); return true; } catch { return false; }
}

function count(text, pattern) {
  return [...text.matchAll(pattern)].length;
}

function requireText(errors, source, marker, label) {
  if (!source.includes(marker)) errors.push(`missing ${label}: ${marker}`);
}

export async function validateUiParity(root = workspace) {
  const errors = [];
  const missing = [];
  for (const [label, relative] of Object.entries(files)) {
    if (!(await exists(resolve(root, relative)))) missing.push(`${label}: ${relative}`);
  }
  if (missing.length) return { errors: missing.map(value => `missing file ${value}`), version: null, items: [], checks: 0 };

  const entries = await Promise.all(Object.entries(files).map(async ([key, relative]) => [key, await readFile(resolve(root, relative), 'utf8')]));
  const source = Object.fromEntries(entries);
  const packageMetadata = JSON.parse(source.package);
  const version = packageMetadata.version;
  const items = ['UI-001', 'UI-002', 'UI-003', 'UI-004', 'UI-005', 'UI-006', 'UI-007'];

  for (const id of items) {
    if (count(source.checklist, new RegExp(`^## ${id}\\b`, 'gm')) !== 1) errors.push(`${id}: checklist heading must appear exactly once`);
  }
  if (source.checklist.includes('- [ ]')) errors.push('checklist contains incomplete acceptance items');
  requireText(errors, source.checklist, `Node Agent \`${version}\``, 'current Node Agent baseline');
  for (const marker of [
    'Actions/OpenAPI', 'FRP and Cloudflare', 'Static Bearer', 'Legacy JSON transport',
    'Per-service runtime supervisor', 'Live history-session running/active/inactive badges',
    'Raw Desktop per-service stdout/stderr'
  ]) requireText(errors, source.checklist, marker, 'intentional exclusion');

  for (const route of [
    '/(telemetry|logs|history', "action === 'telemetry'", "action === 'logs'", "action === 'history'", "action === 'health'", "action === 'diagnostics'"
  ]) requireText(errors, source.managementObservabilityRoute, route, 'Management observability route');
  requireText(errors, source.managementRouter, 'handleManagementObservabilityRoute', 'Management observability router delegation');

  const allowlist = source.observability.match(/const TELEMETRY_RECORD_FIELDS = \[([\s\S]*?)\] as const;/)?.[1] ?? '';
  if (!allowlist) errors.push('telemetry response allowlist is missing');
  for (const forbidden of ['command_preview', 'argument_record', 'resolved_cwd', 'session_id', 'runtime_boot_id', 'arguments_sha256']) {
    if (allowlist.includes(`'${forbidden}'`)) errors.push(`telemetry allowlist exposes forbidden field ${forbidden}`);
  }
  requireText(errors, source.observability, "content.replace(/^\\*\\*Session key:", 'history session-key redaction');
  for (const path of ['/health', '/mcp/info', '/.well-known/oauth-authorization-server', '/.well-known/oauth-protected-resource/mcp']) {
    requireText(errors, source.observability, path, 'fixed health probe');
  }
  if (/fixedProbe\([^\n]*(?:publicBaseUrl|tunnel\.publicUrl)/.test(source.observability)) {
    errors.push('health diagnostics fetch a configured public URL');
  }
  for (const marker of ['validateManagementHealthPayload', 'mcpAuthenticationProbe', 'resource_metadata=']) {
    requireText(errors, source.observability, marker, 'active health contract validation');
  }
  requireText(errors, source.managementObservabilityRoute, 'localListenerBaseUrl(req)', 'socket-derived local health target');
  if (/managementHealthPayload\([^\n]*req\.headers\.host/.test(source.managementObservabilityRoute)) {
    errors.push('health diagnostics trust the HTTP Host header');
  }
  requireText(errors, source.observability, 'canonicalPath(folder.path)', 'canonical history root');
  requireText(errors, source.observability, 'HISTORY_PATH_OUTSIDE_WORKSPACE', 'history symlink escape rejection');
  for (const marker of ['managementOperationLogPayload', 'harnessWorkspaceId(folder.path)', 'safeOperationGroup', 'redactSensitiveText', "'[WORKSPACE]'", 'taskTracked', 'affectedFileCount']) {
    requireText(errors, source.observability, marker, 'structured operation-log contract');
  }
  for (const marker of ['operationResultSummary', 'command_ok', 'verification_ok', 'process_exit_code', 'warning_count']) {
    requireText(errors, source.operationSummary, marker, 'Node bounded operation result summary');
  }
  for (const marker of ['operation_result_summary', 'command_ok', 'verification_ok', 'process_exit_code', 'warning_count']) {
    requireText(errors, source.rustDispatchTracking, marker, 'Rust bounded operation result summary');
  }
  for (const marker of ['mod attachment;', 'mod construction;', 'mod process_lifecycle;', 'mod registry;', 'struct SessionRegistry', 'pub struct SessionStore', 'pub struct ExecSession', 'harness_operation_recorded']) {
    requireText(errors, source.rustSession, marker, 'Rust retained process terminal finalization');
  }
  for (const marker of ['impl ExecSession', 'pub(crate) fn new_with_mode_and_checks', 'child.process_tree_contained()', 'child.kill_hook()', 'pub fn with_execution_identity', 'pub fn with_active_slot', 'pub fn with_sensitive_output', 'pub fn with_telemetry']) {
    requireText(errors, source.rustSessionConstruction, marker, 'Rust session construction and builder boundary');
  }
  for (const marker of ['impl ExecSession', 'pub fn attach_harness_operation', 'lifecycle::record_harness_operation_finalization(self)', 'pub fn operation_id', 'pub fn touch_attachment', 'pub fn mark_detached', 'pub fn is_still_detached']) {
    requireText(errors, source.rustSessionAttachment, marker, 'Rust session attachment and correlation boundary');
  }
  if (/(?:pub fn new_with_mode_and_checks|pub fn with_execution_identity|pub fn with_active_slot|pub fn with_sensitive_output|pub fn with_telemetry|pub fn attach_harness_operation|pub fn touch_attachment|pub fn mark_detached|pub fn is_still_detached)/.test(source.rustSession)) {
    errors.push('Rust session state owner embeds construction or attachment implementation');
  }
  for (const boundary of [source.rustSessionAttachment, source.rustSessionConstruction]) {
    if (/(?:struct ExecSession|struct SessionStore|struct SessionRegistry)/.test(boundary)) {
      errors.push('Rust session construction or attachment boundary defines a second session state owner');
    }
  }
  for (const marker of ['pub(super) fn finish_session', 'record_finalization_telemetry(session)', 'record_harness_operation_finalization(session)', 'session.notify_change()']) {
    requireText(errors, source.rustSessionLifecycle, marker, 'Rust retained process lifecycle finalization');
  }
  for (const marker of ['impl ExecSession', 'pub async fn spawn_readers', 'pub fn spawn_exit_waiter', 'pub async fn wait_for_readers', 'tokio::time::timeout(Duration::from_millis(500), task).await', 'pub async fn kill_and_wait', 'pub async fn wait_for_change', 'pub(super) async fn terminate_process']) {
    requireText(errors, source.rustSessionProcessLifecycle, marker, 'Rust child-process session lifecycle boundary');
  }
  for (const marker of ['status = "verifying";', 'if evicted {', 'store.remove(session_id);']) {
    requireText(errors, source.rustSessionControl, marker, 'Rust finalized-before-eviction session control contract');
  }
  for (const marker of ["status: terminated ? finalized ? killed ? 'killed' : 'exited' : 'verifying' : 'terminating'", 'evicted: terminated && finalized', "until=finalized"]) {
    requireText(errors, source.processDispatcher, marker, 'Node finalized-before-eviction session control contract');
  }
  if (source.processes.includes('if (!session.finalizedAt) await finalizeSession(ctx, session, false);')) {
    errors.push('Node detached/restart control path bypasses normal lifecycle finalization');
  }
  requireText(errors, source.sandboxAppContainer, 'resolveAppContainerPathProgram', 'Node AppContainer child-process-free PATH runtime lookup');
  if (source.sandboxAppContainer.includes("execFileSync('where.exe'")) {
    errors.push('Node AppContainer PATH runtime lookup must not spawn where.exe');
  }
  for (const marker of [
    'const digestFile = `${target}.sha256`;',
    "createHash('sha256').update(await readFile(target)).digest('hex')",
    'writeFile(digestFile, `${digest}\\n`, \'utf8\')'
  ]) {
    requireText(errors, source.appContainerHelperBuild, marker, 'Node AppContainer packaged helper digest manifest');
  }
  for (const marker of [
    'SANDBOX_APPCONTAINER_HELPER_INTEGRITY_FAILED',
    'packagedHelperVerification',
    "createHash('sha256').update(await readFile(candidate)).digest('hex')",
    'if (actualDigest !== expectedDigest)',
    'const packaged = await packagedHelperPath();'
  ]) {
    requireText(errors, source.sandboxAppContainer, marker, 'Node AppContainer packaged helper integrity verification');
  }
  for (const marker of [
    '--completion-marker',
    'TryMarkCompleted(options)',
    'if (!selfCleaned) await cleanupAppContainer',
    'RecordGrant(options, path);',
    'CleanupRecordedGrants(options, sidIdentity)',
    'GetSecurityDescriptorSddlForm(AccessControlSections.Access)',
    'SetSecurityDescriptorSddlForm(sddl, AccessControlSections.Access)',
    'FileSecurity security = new FileInfo(path).GetAccessControl(AccessControlSections.Access)',
    'ProtectLeaseDirectory(options, sidIdentity);',
    'AccessControlType.Deny',
    'EnsureGrantDoesNotCoverLeaseDirectory(options, path);',
    'EnsureGrantDoesNotTargetProtectedMetadata(options, path);'
  ]) {
    requireText(errors, source.sandboxAppContainer, marker, 'Node AppContainer self-cleanup and crash-recovery contract');
  }
  const nodeManagedSidParses = source.sandboxAppContainer.match(/new SecurityIdentifier\(SidText\(sid\)\)/g)?.length ?? 0;
  if (nodeManagedSidParses !== 2) {
    errors.push(`Node AppContainer must parse the managed SecurityIdentifier exactly once for run and once for fallback cleanup; found ${nodeManagedSidParses}`);
  }
  if (source.sandboxAppContainer.includes('new SecurityIdentifier(sidText)')) {
    errors.push('Node AppContainer ACL helpers must reuse the managed SecurityIdentifier instead of reparsing SID text');
  }
  for (const marker of [
    'bool profileCreated = false;',
    'profileCreated = true;',
    'if (profileCreated) {'
  ]) {
    requireText(errors, source.sandboxAppContainer, marker, 'Node AppContainer profile ownership contract');
  }
  const nodeCreateFailureCheck = source.sandboxAppContainer.indexOf('if (result < 0) throw new InvalidOperationException("CreateAppContainerProfile failed: 0x"');
  const nodeProfileOwned = source.sandboxAppContainer.indexOf('profileCreated = true;', nodeCreateFailureCheck);
  if (nodeCreateFailureCheck < 0 || nodeProfileOwned < nodeCreateFailureCheck) {
    errors.push('Node AppContainer must mark profile ownership only after CreateAppContainerProfile succeeds');
  }
  requireText(errors, source.sandboxAppContainer, 'class ProtectedAclTarget', 'Node AppContainer protected ACL snapshot reuse');
  requireText(errors, source.sandboxAppContainer, 'FileSystemSecurity security = target.Security;', 'Node AppContainer protected ACL snapshot reuse');
  const nodeProtectedStart = source.sandboxAppContainer.indexOf('private static void RestrictProtected(');
  const nodeProtectedEnd = source.sandboxAppContainer.indexOf('private static bool RestoreProtected(', nodeProtectedStart);
  const nodeProtectedBody = nodeProtectedStart >= 0 && nodeProtectedEnd > nodeProtectedStart
    ? source.sandboxAppContainer.slice(nodeProtectedStart, nodeProtectedEnd)
    : '';
  const nodeProtectedAclReads = nodeProtectedBody.match(/GetAccessControl\(AccessControlSections\.Access\)/g)?.length ?? 0;
  if (nodeProtectedAclReads !== 2) {
    errors.push(`Node RestrictProtected must read each directory/file ACL branch once before journaling; found ${nodeProtectedAclReads} GetAccessControl calls`);
  }
  for (const marker of [
    'IsProtectedMetadataRoot(Options options, string value)',
    'RemoveGrants(options, sidIdentity, granted)',
    'bool restored = RestoreProtected(options);',
    'if (!restored) {'
  ]) {
    requireText(errors, source.sandboxAppContainer, marker, 'Node AppContainer optimized protected cleanup contract');
  }
  const nodeRemoveGrantsStart = source.sandboxAppContainer.indexOf('private static bool RemoveGrants(Options options, SecurityIdentifier sid, List<string> paths)');
  const nodeRemoveGrantsEnd = source.sandboxAppContainer.indexOf('private static bool RemoveSid(', nodeRemoveGrantsStart);
  const nodeRemoveGrantsBody = nodeRemoveGrantsStart >= 0 && nodeRemoveGrantsEnd > nodeRemoveGrantsStart
    ? source.sandboxAppContainer.slice(nodeRemoveGrantsStart, nodeRemoveGrantsEnd)
    : '';
  const nodeParentGrantRemoval = nodeRemoveGrantsBody.indexOf('else removed = RemoveSid(path, sid) && removed;');
  const nodeProtectedRestore = nodeRemoveGrantsBody.indexOf('bool restored = RestoreProtected(options);');
  if (nodeParentGrantRemoval < 0 || nodeProtectedRestore < 0 || nodeParentGrantRemoval > nodeProtectedRestore) {
    errors.push('Node AppContainer must remove parent/runtime grants before restoring protected metadata inheritance');
  }
  const protectedBeforeWorkspaceGrant = source.sandboxAppContainer.indexOf('RestrictProtected(options, sidIdentity, granted);');
  const workspaceGrant = source.sandboxAppContainer.indexOf('ApplyGrant(options, options.Workspace, sidIdentity, "modify", granted);');
  if (protectedBeforeWorkspaceGrant < 0 || workspaceGrant < 0 || protectedBeforeWorkspaceGrant > workspaceGrant) {
    errors.push('Node AppContainer must snapshot/protect .git/.github before granting inheritable workspace modify access');
  }
  for (const marker of [
    'APPCONTAINER_PROTECTED_METADATA_CAPABILITY_PREFIX',
    'APPCONTAINER_WORKSPACE_MODIFY_CAPABILITY_PREFIX',
    'protected_metadata_capability_name(workspace.root())?',
    'workspace_modify_capability_name(workspace.root())?',
    'fn protected_metadata_acl_has_required_grant(',
    'fn ensure_persistent_protected_metadata_grant(',
    'fn ensure_persistent_workspace_grant(',
    'fn publish_persistent_acl_marker<F>(',
    'protected_metadata_capability_is_stable_and_workspace_scoped'
  ]) {
    requireText(errors, source.rustAppContainerProvider, marker, 'Rust AppContainer persistent workspace/metadata capability contract');
  }
  const rustProtectedBeforeWorkspaceGrant = source.rustAppContainerProvider.indexOf('prepare_protected_asset_restrictions(');
  const rustWorkspaceGrant = source.rustAppContainerProvider.indexOf('ensure_persistent_workspace_grant(');
  if (rustProtectedBeforeWorkspaceGrant < 0 || rustWorkspaceGrant < 0 || rustProtectedBeforeWorkspaceGrant > rustWorkspaceGrant) {
    errors.push('Rust AppContainer must install the persistent .git/.github read-only boundary before the persistent workspace Modify grant');
  }
  requireText(
    errors,
    source.rustAppContainerProvider,
    'merge_and_install_acl_via_handle(&path, dacl, entry, was_protected)',
    'Rust AppContainer handle-based ACL grant application'
  );
  if (source.rustAppContainerProvider.includes('set_trustee_access(&path, sid, REVOKE_ACCESS, 0, NO_INHERITANCE, true)?;')) {
    errors.push('Rust protected repository ACL setup must not revoke immediately before SET_ACCESS');
  }
  for (const marker of [
    'struct SharedSid',
    'SharedSid::copy_from(profile.sid())?',
    'self.sid.sid()'
  ]) {
    requireText(errors, source.rustAppContainerProvider, marker, 'Rust AppContainer shared SID cleanup contract');
  }
  if (source.rustAppContainerProvider.includes('let sid_value = HSTRING::from(self.sid.as_ref());')) {
    errors.push('Rust AppContainer ACL cleanup must not reparse the SID for each grant');
  }
  for (const marker of [
    'static APPCONTAINER_ID_SEQUENCE: AtomicU64',
    'fetch_add(1, Ordering::Relaxed)',
    'let identity = appcontainer_identity_suffix()?;',
    'appcontainer_identity_is_unique_for_the_same_timestamp'
  ]) {
    requireText(errors, source.rustAppContainerProvider, marker, 'Rust AppContainer collision-resistant identity contract');
  }
  if (source.rustAppContainerProvider.includes('let nonce = SystemTime::now()')) {
    errors.push('Rust AppContainer identities must not rely on a timestamp-only nonce');
  }
  requireText(errors, source.rustSessionControl, 'use super::process_lifecycle::terminate_process;', 'Rust session signal termination delegation');
  if (/(?:pub async fn spawn_readers|pub fn spawn_exit_waiter|pub async fn wait_for_readers|pub async fn kill_and_wait|pub async fn wait_for_change|async fn terminate_process)/.test(source.rustSession)) {
    errors.push('Rust session state owner embeds child-process lifecycle implementation');
  }
  if (/(?:struct ExecSession|struct SessionStore|struct SessionRegistry)/.test(source.rustSessionProcessLifecycle)) {
    errors.push('Rust child-process lifecycle boundary defines a second session state owner');
  }
  for (const marker of ['impl Default for SessionStore', 'impl SessionStore', 'pub async fn acquire_active_slot', 'prune_finalized_sessions(&mut registry)', 'pub fn get_by_operation', 'pub fn remove']) {
    requireText(errors, source.rustSessionRegistry, marker, 'Rust session registry and admission boundary');
  }
  if (/(?:impl Default for SessionStore|impl SessionStore)/.test(source.rustSession)) {
    errors.push('Rust session state owner embeds registry and admission implementation');
  }
  if (/(?:struct ExecSession|struct SessionStore|struct SessionRegistry)/.test(source.rustSessionRegistry)) {
    errors.push('Rust session registry boundary defines a second session state owner');
  }
  for (const marker of ['mod admission;', 'mod identity;', 'mod lifecycle;', 'mod native_diagnostic;', 'mod post_check;', 'mod request;', 'use admission::{admit_operation, OperationAdmission};', 'use identity::execution_identity;', 'use lifecycle::run_command;', 'use native_diagnostic::run_native_diagnostic;', 'use request::{resolve_exec_request, resolve_runtime_options};']) {
    requireText(errors, source.rustExec, marker, 'Rust exec request facade');
  }
  for (const marker of ['pub(super) struct ResolvedExecRequest', "pub(super) struct ExecRuntimeOptions<'a>", 'pub(super) fn resolve_exec_request', 'pub(super) fn resolve_runtime_options', 'fn resolved_command_timeout_ms', 'fn validate_child_process_scope']) {
    requireText(errors, source.rustExecRequest, marker, 'Rust exec request resolution boundary');
  }
  for (const marker of ['pub(super) enum OperationAdmission', 'pub(super) async fn admit_operation', 'ctx.sessions.get_by_operation(operation_id)', 'let automatic_operation = operation_id.starts_with("auto:");', 'let reuse_session = !automatic_operation || !session.is_finalized();', 'ctx.sessions.remove(&session.session_id);', 'session.touch_attachment()', 'OPERATION_ID_CONFLICT']) {
    requireText(errors, source.rustExecAdmission, marker, 'Rust exec operation admission boundary');
  }
  requireText(errors, source.rustExecResult, 'pub(super) fn attach_session_capacity', 'Rust exec result capacity boundary');
  if (/(?:fn resolved_command_timeout_ms|fn validate_child_process_scope|let automatic_operation =|let reuse_session =|ctx\.sessions\.get_by_operation|session\.touch_attachment\(\))/.test(source.rustExec)) {
    errors.push('Rust exec request facade embeds request resolution or operation admission implementation');
  }
  if (/(?:ctx\.sessions|resource_lock|OwnedMutexGuard)/.test(source.rustExecRequest)) {
    errors.push('Rust exec request resolution boundary owns operation admission state');
  }
  for (const boundary of [source.rustExecRequest, source.rustExecAdmission]) {
    if (/(?:spawn_with_|prepared_command|ExecSession::new_with_mode_and_checks|acquire_start_permission|spawn_lifecycle_monitor)/.test(boundary)) {
      errors.push('Rust exec request or admission boundary owns main-process lifecycle implementation');
    }
  }
  for (const marker of ['pub(super) struct ExecutionIdentity', 'pub(super) fn cargo_target_lock', 'pub(super) fn execution_identity']) {
    requireText(errors, source.rustExecIdentity, marker, 'Rust exec identity and resource-lock boundary');
  }
  for (const marker of ['struct RequestCancellationGuard', 'pub(super) async fn run_command', 'start_exec_process(&backend, spec, cwd, CommandIoMode::Session).await', 'ExecSession::new_with_mode_and_checks', 'with_sandbox_phase_durations', 'fn spawn_lifecycle_monitor', 'run_post_checks(post_checks, &cwd, &backend).await', 'session.release_backend_lifetimes().await', 'set_sandbox_cleanup_ms']) {
    requireText(errors, source.rustExecLifecycle, marker, 'Rust main-process lifecycle boundary');
  }
  if (source.rustExecLifecycle.includes('session.mark_termination_reason("detached_timeout");')
      && source.rustExecLifecycle.includes('session.mark_finalized();')) {
    const detachedSection = source.rustExecLifecycle.slice(
      source.rustExecLifecycle.indexOf('session.mark_termination_reason("detached_timeout");'),
      source.rustExecLifecycle.indexOf('pub(super) async fn run_command')
    );
    if (detachedSection.includes('session.mark_finalized();')) {
      errors.push('Rust detached timeout path bypasses normal lifecycle finalization');
    }
  }
  for (const marker of ['pub(super) async fn start_exec_process', 'spawn_with_control', 'start_prepared_sandbox_command']) {
    requireText(errors, source.rustExecBackend, marker, 'Rust execution backend startup boundary');
  }
  for (const marker of ['pub(crate) async fn spawn_with_control', 'acquire_start_permission().await']) {
    requireText(errors, source.rustProcessStart, marker, 'Rust process startup controller boundary');
  }
  if (/(?:struct RequestCancellationGuard|async fn run_command\(|fn spawn_lifecycle_monitor\(|spawn_with_permission|acquire_start_permission)/.test(source.rustExec)) {
    errors.push('Rust exec request facade owns main-process lifecycle implementation');
  }
  if (/(?:pub async fn exec_command_async|pub fn exec_health_check)/.test(source.rustExecLifecycle)) {
    errors.push('Rust exec lifecycle boundary owns a public tool facade');
  }
  for (const marker of ['pub(super) fn run_native_diagnostic', 'ctx.workspace.resolve_existing(path)?.path', '"execution_mode": "native_builtin"', '"command_runner": "native_builtin"']) {
    requireText(errors, source.rustExecNativeDiagnostic, marker, 'Rust child-process-free native diagnostic boundary');
  }
  if (/(?:spawn_with_|prepared_command|Command::new)/.test(source.rustExecNativeDiagnostic)) {
    errors.push('Rust native diagnostic boundary must not create child processes');
  }
  for (const marker of ['pub(super) async fn run_post_checks', 'start_exec_process', 'error.to_error_value()', '"execution_mode": "parallel"']) {
    requireText(errors, source.rustExecPostCheck, marker, 'Rust post-check execution boundary');
  }
  for (const marker of ['mod pool_policy;', 'use pool_policy::{']) {
    requireText(errors, source.rustBuiltinTunnel, marker, 'Rust built-in tunnel pool policy delegation');
  }
  for (const marker of [
    'pub(super) const MAX_RECONNECT_DELAY',
    'pub(super) const INITIAL_RECONNECT_DELAY',
    'pub(super) struct PoolCounts',
    'pub(super) struct PoolAdjustment',
    'pub(super) enum ScaleUpBlock',
    'pub(super) fn configured_max_connecting',
    'pub(super) fn configured_burst_warm_floor',
    'pub(super) fn pool_adjustment',
    'pub(super) fn scale_up_reason',
    'pub(super) fn scale_down_reason',
    'pub(super) fn scale_up_block',
    'pub(super) fn join_worker_indices',
    'pub(super) fn jittered_limit',
    'pub(super) fn worker_should_recycle',
    'pub(super) fn next_reconnect_base',
    'pub(super) fn reconnect_delay'
  ]) {
    requireText(errors, source.rustBuiltinPoolPolicy, marker, 'Rust built-in tunnel pure pool policy boundary');
  }
  if (/(?:const (?:MAX_RECONNECT_DELAY|INITIAL_RECONNECT_DELAY)|struct (?:PoolCounts|PoolAdjustment)|enum ScaleUpBlock|fn (?:configured_max_connecting|configured_burst_warm_floor|pool_adjustment|scale_up_reason|scale_down_reason|scale_up_block|join_worker_indices|jittered_limit|worker_should_recycle|next_reconnect_base|reconnect_delay))/.test(source.rustBuiltinTunnel)) {
    errors.push('Rust built-in tunnel transport owner embeds pure pool policy implementation');
  }
  if (/(?:connect_async|WebSocketStream|receive_request|forward_request|load_or_enroll_device_identity|mpsc|watch::|JoinSet|TcpStream)/.test(source.rustBuiltinPoolPolicy)) {
    errors.push('Rust built-in tunnel pool policy boundary owns transport, enrollment, or worker lifecycle implementation');
  }
  for (const marker of ['mod request_mapping;', 'use request_mapping::{']) {
    requireText(errors, source.rustBuiltinTunnel, marker, 'Rust built-in tunnel request mapping delegation');
  }
  for (const marker of [
    'pub(super) struct IncomingRequest',
    'pub(super) fn prepare_local_request',
    'pub(super) fn response_headers',
    'pub(super) fn local_path_for_request'
  ]) {
    requireText(errors, source.rustBuiltinRequestMapping, marker, 'Rust built-in tunnel request mapping boundary');
  }
  if (/(?:struct IncomingRequest|fn (?:local_path_for_request|response_headers)|reqwest::Method::from_bytes|HeaderName::from_bytes)/.test(source.rustBuiltinTunnel)) {
    errors.push('Rust built-in tunnel transport owner embeds request mapping implementation');
  }
  if (/(?:async fn|ClientSink|ClientStream|WebSocketStream|Message::|connect_async|send_control|receive_control|receive_live_control|mpsc|watch::|JoinSet|TcpStream|tokio::select!|bytes_stream)/.test(source.rustBuiltinRequestMapping)) {
    errors.push('Rust built-in tunnel request mapping boundary owns transport, control I/O, or worker lifecycle implementation');
  }
  for (const marker of ['mod protocol_io;', 'use protocol_io::{']) {
    requireText(errors, source.rustBuiltinTunnel, marker, 'Rust built-in tunnel protocol I/O delegation');
  }
  for (const marker of [
    'const HEARTBEAT_TIMEOUT',
    'const WEBSOCKET_CLOSE_TIMEOUT',
    'pub(super) type ClientWebSocket',
    'pub(super) type ClientSink',
    'pub(super) type ClientStream',
    'pub(super) struct HeartbeatTracker',
    'pub(super) fn decode_control',
    'pub(super) fn encode_control',
    'pub(super) async fn close_client_websocket',
    'pub(super) async fn send_heartbeat',
    'pub(super) async fn receive_control',
    'pub(super) async fn send_control',
    'return decode_control(text.as_str())',
    'let encoded = encode_control(message)?'
  ]) {
    requireText(errors, source.rustBuiltinProtocolIo, marker, 'Rust built-in tunnel protocol I/O boundary');
  }
  if (/(?:const (?:HEARTBEAT_TIMEOUT|WEBSOCKET_CLOSE_TIMEOUT)|type (?:ClientWebSocket|ClientSink|ClientStream)|struct HeartbeatTracker|fn (?:close_client_websocket|send_heartbeat|receive_control|send_control)|serde_json::from_str\(text\.as_str\(\)\)|serde_json::to_string\(message\))/.test(source.rustBuiltinTunnel)) {
    errors.push('Rust built-in tunnel lifecycle owner embeds low-level protocol I/O implementation');
  }
  if (/(?:BuiltinTunnelConfig|WorkerPolicy|WorkerEvent|PoolWorkerState|IncomingRequest|prepare_local_request|forward_request|run_connected_worker|worker_reconnect_loop|load_or_enroll_device_identity|connect_async|mpsc|watch::|JoinSet|reqwest|tokio::select!|bytes_stream)/.test(source.rustBuiltinProtocolIo)) {
    errors.push('Rust built-in tunnel protocol I/O boundary owns worker, forwarding, enrollment, or cancellation lifecycle implementation');
  }
  for (const marker of ['mod connection;', 'use connection::{']) {
    requireText(errors, source.rustBuiltinTunnel, marker, 'Rust built-in tunnel authenticated connection delegation');
  }
  for (const marker of [
    'pub(super) struct AuthenticatedWorkerConnection',
    'pub(super) async fn connect_authenticated_worker',
    'pub(super) fn unix_ms',
    'WEBSOCKET_CONNECT_TIMEOUT',
    'CLIENT_ID_HEADER',
    'SERVICE_HEADER',
    'SEC_WEBSOCKET_PROTOCOL',
    'ControlMessage::Challenge',
    'ControlMessage::Authenticate',
    'ControlMessage::HelloAck',
    'auth_signing_payload'
  ]) {
    requireText(errors, source.rustBuiltinConnection, marker, 'Rust built-in tunnel authenticated connection boundary');
  }
  requireText(errors, source.rustBuiltinTunnel, 'const WEBSOCKET_CONNECT_TIMEOUT', 'Rust built-in tunnel shared connection timeout');
  if (/(?:fn unix_ms|IntoClientRequest|connect_async|CLIENT_ID_HEADER|SERVICE_HEADER|auth_signing_payload|DeviceAuthProof|ClientHello)/.test(source.rustBuiltinTunnel)) {
    errors.push('Rust built-in tunnel lifecycle owner embeds authenticated connection handshake implementation');
  }
  if (/(?:watch::|mpsc|JoinSet|WorkerEvent|PoolWorkerState|WorkerConnectionExit|ConnectedWorkerGuard|BuiltinTunnelMetrics|forward_request|receive_request|receive_live_control|prepare_local_request|response_headers|reqwest|bytes_stream|tokio::select!|load_or_enroll_device_identity|policy_tx|status_tx|event_tx|shutdown|retire|ControlMessage::Ready)/.test(source.rustBuiltinConnection)) {
    errors.push('Rust built-in tunnel authenticated connection boundary owns worker, forwarding, policy publication, or cancellation lifecycle implementation');
  }
  if (!/connect_authenticated_worker\(config, worker_id\)\.await\?;[\s\S]*?policy_tx\.send_replace\(Some\(initial_policy\.clone\(\)\)\);[\s\S]*?send_control\(&mut sink, &ControlMessage::Ready\)\.await\?;[\s\S]*?\*connected = true;/.test(source.rustBuiltinTunnel)) {
    errors.push('Rust built-in tunnel parent must publish the authenticated policy before Ready and connected state');
  }
  for (const marker of ['mod metrics;', 'pub use metrics::BuiltinTunnelSnapshot;', 'use metrics::{BuiltinTunnelMetrics, ConnectedWorkerGuard};']) {
    requireText(errors, source.rustBuiltinTunnel, marker, 'Rust built-in tunnel metrics delegation');
  }
  for (const marker of [
    'pub struct BuiltinTunnelSnapshot',
    'pub fn availability_state',
    'pub(super) struct BuiltinTunnelMetrics',
    'pub(super) fn new',
    'pub(super) fn set_policy',
    'pub(super) fn set_pool_counts',
    'pub(super) fn record_recycle',
    'pub(super) fn set_last_error',
    'pub(super) fn snapshot',
    'pub(super) struct ConnectedWorkerGuard',
    'impl Drop for ConnectedWorkerGuard'
  ]) {
    requireText(errors, source.rustBuiltinMetrics, marker, 'Rust built-in tunnel metrics boundary');
  }
  if (/(?:struct BuiltinTunnelSnapshot|struct BuiltinTunnelMetrics|struct ConnectedWorkerGuard|impl BuiltinTunnelSnapshot|impl BuiltinTunnelMetrics|impl ConnectedWorkerGuard|impl Drop for ConnectedWorkerGuard)/.test(source.rustBuiltinTunnel)) {
    errors.push('Rust built-in tunnel lifecycle owner embeds metrics state implementation');
  }
  if (/(?:watch::|mpsc|JoinHandle|JoinSet|BuiltinTunnelHandle|BuiltinTunnelConfig|WorkerEvent|PoolWorkerState|WorkerConnectionExit|run_worker_pool|run_connected_worker|worker_reconnect_loop|forward_request|receive_request|receive_live_control|send_control|ControlMessage|reqwest|tokio::select!|shutdown|retire)/.test(source.rustBuiltinMetrics)) {
    errors.push('Rust built-in tunnel metrics boundary owns worker, transport, channel, task, or cancellation lifecycle implementation');
  }
  requireText(errors, source.rustBuiltinTunnel, 'mod tests;', 'Rust built-in tunnel external regression suite');
  for (const marker of [
    'use super::*;',
    'fn parses_namespaced_mcp_endpoint',
    'fn dynamic_pool_plan_uses_demand_connecting_limits_and_staged_shrink',
    'async fn worker_pool_bootstraps_grows_and_gracefully_shrinks_from_server_policy',
    'async fn worker_recycles_after_request_limit_and_pool_replaces_it',
    'async fn worker_pool_reconnects_after_authenticated_socket_closes'
  ]) {
    requireText(errors, source.rustBuiltinTests, marker, 'Rust built-in tunnel external regression suite');
  }
  if (/(?:mod tests\s*\{|#\[(?:tokio::)?test\])/.test(source.rustBuiltinTunnel)) {
    errors.push('Rust built-in tunnel lifecycle owner embeds its regression suite');
  }
  for (const marker of ['deferred_process_operation', 'session.attach_harness_operation']) {
    requireText(errors, source.rustDispatchTracking, marker, 'Rust async operation correlation regression');
  }
  requireText(errors, source.rustDispatch, 'vec!["started", "failed"]', 'Rust async operation provisional-status regression');
  if (/operationResultSummary[\s\S]*?\.\.\.result/.test(source.operationSummary)) errors.push('Node operation result summary spreads the raw result');
  if (/operation_result_summary[\s\S]*?\.extend\(/.test(source.rustDispatch)) errors.push('Rust operation result summary extends from a raw result object');
  for (const marker of ['attachHarnessOperation', 'deferredProcessOperation', 'result.command_ok === null']) {
    requireText(errors, source.taskTools, marker, 'deferred process operation binding');
  }
  for (const marker of ['recordHarnessOperationFinalization', 'session.harnessOperations', 'session.harnessOperationRecordedIds', 'attachHarnessOperation']) {
    requireText(errors, source.processHarnessTracking, marker, 'retained process terminal finalization tracking');
  }
  requireText(errors, source.processes, 'await recordHarnessOperationFinalization(ctx, session)', 'retained process lifecycle finalization bridge');
  requireText(errors, source.observability, 'operationRecordIsTerminal', 'legacy provisional operation handling');
  const operationResponse = source.observability.match(/function safeOperationGroup[\s\S]*?return \{([\s\S]*?)\r?\n  \};\r?\n\}/)?.[1] ?? '';
  if (!operationResponse) errors.push('operation-log derived response contract is missing');
  for (const forbidden of ['workspace_id', 'task_id', 'input_summary', 'result_summary', 'affected_files']) {
    if (new RegExp(`\\b${forbidden}\\s*:`).test(operationResponse)) errors.push(`operation-log response exposes raw field ${forbidden}`);
  }

  const workspaceTabType = source.workspacePage.match(/type WorkspaceTab = ([^;]+);/)?.[1] ?? '';
  if (!workspaceTabType) errors.push('workspace tab type contract is missing');
  for (const tab of ['overview', 'history', 'telemetry', 'logs', 'health', 'settings']) {
    if (!workspaceTabType.includes(`"${tab}"`)) errors.push(`workspace tab type is missing core tab ${tab}`);
  }
  for (const component of ['HistoryViewer', 'TelemetryViewer', 'OperationLogViewer', 'HealthPanel']) {
    requireText(errors, source.workspacePage, component, 'workspace observability surface');
  }
  requireText(errors, source.workspacePage, 'role="tabpanel"', 'accessible workspace tab contract');
  requireText(errors, source.workspacePage, 'capabilities.operationLogs', 'operation-log capability gate');
  requireText(errors, source.workspacePage, 'capabilities.actions', 'actions capability gate');
  for (const marker of ['allowedCommands', 'workspaceLocalEntries', 'workspaceScriptExtensions', 'blockingAdmissionLimit', 'processAdmissionLimit']) {
    requireText(errors, source.runtimePolicyForm, marker, 'fine-grained policy field');
  }
  requireText(errors, source.frontendCapabilities, 'agentRestart: true', 'node agent-restart capability');
  requireText(errors, source.frontendCapabilities, 'rawRuntimeLogs: false', 'node raw-log exclusion');
  requireText(errors, source.nodeBackend, '/admin/api/workspaces/', 'node admin API mapping');
  requireText(errors, source.nodeBackend, 'x-ctmcp-admin-token', 'node admin token header');
  requireText(errors, source.nodeBackend, "credentials: \"same-origin\"", 'node same-origin credentials');
  requireText(errors, source.nodeBackend, 'cache: "no-store"', 'node no-store fetches');
  requireText(errors, source.managementUi, "script-src 'self'", 'management CSP script-src');
  requireText(errors, source.managementUi, "default-src 'none'", 'management CSP default-src');
  if (source.managementUi.includes('unsafe-inline')) {
    errors.push('management UI CSP allows unsafe-inline');
  }
  requireText(errors, source.managementUi, 'ctmcp-admin-token', 'admin token HTML injection');
  requireText(errors, source.telemetryViewer, 'readWorkspaceTelemetry', 'shared telemetry viewer');
  requireText(errors, source.operationLogViewer, 'operations.query', 'shared operation-log viewer');
  requireText(errors, source.operationLogViewer, 'nextCursor', 'paged operation-log cursor');
  requireText(errors, source.historyViewer, 'listHistorySessions', 'shared history viewer');
  requireText(errors, source.healthPanel, 'runHealthChecks', 'shared health panel');

  requireText(errors, source.managementTest, 'management observability routes expose sanitized telemetry, operation logs, history, health and diagnostics', 'observability integration test');
  requireText(errors, source.managementTest, 'shared Svelte observability surfaces stay on the frontend backend contract', 'shared Svelte observability regression test');
  requireText(errors, source.managementTest, 'management health validators reject incomplete local metadata contracts', 'health contract validation test');
  requireText(errors, source.managementTest, 'mcpChallenge?.status', 'active MCP OAuth challenge regression');
  requireText(errors, source.harnessTest, 'operation logs persist bounded execution diagnostics without raw process payloads', 'persisted operation diagnostics regression');
  for (const marker of ["yield_time_ms: 0", "['failed', 'started']"]) {
    requireText(errors, source.harnessTest, marker, 'async process terminal correlation regression');
  }
  for (const marker of ['TELEMETRY_COMMAND_SECRET', 'OP_LOG_REASON_MARKER', 'OP_LOG_COMMAND_MARKER', 'OP_LOG_MULTILINE_TAIL', 'OPERATION_COMMAND_SECRET', 'COMMAND_FAILED', "status: 'running'", 'process_exit_code', 'warning_count', 'HISTORY_SESSION_KEY_SECRET', 'HISTORY_PATH_OUTSIDE_WORKSPACE', "host: 'localhost:9'", 'globalBlockingConcurrency', 'workspaceLocalEntries']) {
    requireText(errors, source.managementTest, marker, 'security and policy regression coverage');
  }

  return { errors, version, items, checks: items.length };
}

if (process.argv[1] && resolve(process.argv[1]) === scriptPath) {
  const result = await validateUiParity();
  if (result.errors.length) {
    for (const error of result.errors) console.error(`ERROR: ${error}`);
    process.exitCode = 1;
  } else {
    console.log(`Node Agent UI parity: ${result.checks}/${result.items.length} complete; version ${result.version}`);
  }
}
