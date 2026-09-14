import path from 'node:path';
import type { JsonObject } from '../types.js';
import { ProcessToolError } from './errors.js';

interface TestCommandSpec {
  program: string;
  args: string[];
}

interface TestRunnerDescriptor {
  runner: 'codeception' | 'phpunit' | 'pytest' | 'cargo' | 'node';
  helpArgs: string[];
  selectorSyntax: string;
}

interface TestRunnerCapabilities {
  runner: TestRunnerDescriptor['runner'];
  probeOk: boolean;
  supportsFilter: boolean;
  supportsTestNamePattern: boolean;
  selectorSyntax: string;
}

export interface TestRunnerCapabilityProbeResult {
  code: number | null;
  stdout: string;
  stderr: string;
}

export type TestRunnerCapabilityProbe = (
  program: string,
  args: string[],
  cwd: string
) => Promise<TestRunnerCapabilityProbeResult>;

const capabilityCache = new Map<string, TestRunnerCapabilities>();

function executableStem(value: string): string {
  const normalized = value.replaceAll('/', '\\');
  return path.win32.basename(normalized).replace(/\.(?:exe|cmd|bat|phar)$/i, '').toLowerCase();
}

function wrapperRunner(args: string[], wrapper: string, candidate: string): { prefix: string[]; remaining: string[] } | undefined {
  if (executableStem(wrapper) !== 'php' || !args.length || executableStem(args[0]) !== candidate) return undefined;
  return { prefix: [args[0]], remaining: args.slice(1) };
}

export function describeTestRunner(spec: TestCommandSpec): TestRunnerDescriptor | undefined {
  const program = executableStem(spec.program);
  let prefix: string[] = [];
  let remaining = spec.args;
  let runner: TestRunnerDescriptor['runner'] | undefined;
  let selectorSyntax = '';

  if (program === 'codecept') {
    runner = 'codeception';
    selectorSyntax = 'tests/path/Test.php:testMethod';
  } else if (program === 'phpunit') {
    runner = 'phpunit';
    selectorSyntax = '--filter <pattern>';
  } else {
    const codecept = wrapperRunner(spec.args, spec.program, 'codecept');
    const phpunit = wrapperRunner(spec.args, spec.program, 'phpunit');
    if (codecept) {
      runner = 'codeception';
      selectorSyntax = 'tests/path/Test.php:testMethod';
      prefix = codecept.prefix;
      remaining = codecept.remaining;
    } else if (phpunit) {
      runner = 'phpunit';
      selectorSyntax = '--filter <pattern>';
      prefix = phpunit.prefix;
      remaining = phpunit.remaining;
    } else if (program === 'pytest' || program === 'py.test') {
      runner = 'pytest';
      selectorSyntax = '-k <expression> or path::test_name';
    } else if ((program === 'python' || program === 'python3') && spec.args[0] === '-m' && spec.args[1] === 'pytest') {
      runner = 'pytest';
      selectorSyntax = '-k <expression> or path::test_name';
      prefix = ['-m', 'pytest'];
      remaining = spec.args.slice(2);
    } else if (program === 'cargo' && spec.args[0] === 'test') {
      runner = 'cargo';
      selectorSyntax = 'cargo test <test-name>';
      prefix = ['test'];
      remaining = spec.args.slice(1);
    } else if ((program === 'node' || program === 'nodejs') && spec.args.includes('--test')) {
      runner = 'node';
      selectorSyntax = '--test-name-pattern <pattern>';
    }
  }

  if (!runner) return undefined;
  const helpArgs = [...prefix];
  if (runner === 'codeception' && remaining.includes('run')) helpArgs.push('run');
  helpArgs.push('--help');
  return { runner, helpArgs, selectorSyntax };
}

function requestedSelector(args: readonly string[]): string | undefined {
  return args.find(arg => arg === '--filter' || arg.startsWith('--filter=')
    || arg === '--test-name-pattern' || arg.startsWith('--test-name-pattern='));
}

type TestWorkflowStage = 'focused' | 'affected' | 'full';

const CARGO_VALUE_OPTIONS = new Set([
  '--manifest-path', '--package', '-p', '--exclude', '--features', '--target', '--target-dir',
  '--jobs', '-j', '--profile', '--color', '--message-format', '--config', '--test', '--bin',
  '--example', '--bench'
]);
const CARGO_TARGET_FLAGS = new Set([
  '--lib', '--bins', '--tests', '--benches', '--examples', '--all-targets', '--doc'
]);
const CARGO_TARGET_VALUE_OPTIONS = new Set(['--test', '--bin', '--example', '--bench']);
const NODE_TEST_VALUE_OPTIONS = new Set([
  '--test-concurrency', '--test-name-pattern', '--test-reporter', '--test-reporter-destination',
  '--test-shard', '--test-timeout', '--test-isolation'
]);

function removeOptionWithValue(args: readonly string[], option: string): string[] {
  const result: string[] = [];
  for (let index = 0; index < args.length; index += 1) {
    const value = args[index];
    if (value === option) {
      index += 1;
      continue;
    }
    if (value.startsWith(`${option}=`)) continue;
    result.push(value);
  }
  return result;
}

function cargoFilterIndex(args: readonly string[]): number | undefined {
  const separator = args.indexOf('--');
  const end = separator < 0 ? args.length : separator;
  for (let index = 1; index < end; index += 1) {
    const value = args[index];
    if (value.startsWith('-')) {
      if (CARGO_VALUE_OPTIONS.has(value)) index += 1;
      continue;
    }
    return index;
  }
  return undefined;
}

function cargoHasTargetScope(args: readonly string[]): boolean {
  const separator = args.indexOf('--');
  const end = separator < 0 ? args.length : separator;
  for (let index = 1; index < end; index += 1) {
    const value = args[index];
    if (CARGO_TARGET_FLAGS.has(value) || CARGO_TARGET_VALUE_OPTIONS.has(value)
      || [...CARGO_TARGET_VALUE_OPTIONS].some(option => value.startsWith(`${option}=`))) return true;
    if (CARGO_VALUE_OPTIONS.has(value)) index += 1;
  }
  return false;
}

function stripCargoTargetScope(args: readonly string[]): string[] {
  const result: string[] = [];
  const separator = args.indexOf('--');
  const end = separator < 0 ? args.length : separator;
  for (let index = 0; index < end; index += 1) {
    const value = args[index];
    if (CARGO_TARGET_FLAGS.has(value)) continue;
    if (CARGO_TARGET_VALUE_OPTIONS.has(value)) {
      index += 1;
      continue;
    }
    if ([...CARGO_TARGET_VALUE_OPTIONS].some(option => value.startsWith(`${option}=`))) continue;
    result.push(value);
  }
  if (separator >= 0) result.push(...args.slice(separator));
  return result;
}

function nodeTestFileIndexes(args: readonly string[]): number[] {
  const testIndex = args.indexOf('--test');
  if (testIndex < 0) return [];
  const files: number[] = [];
  let consumesValue = false;
  for (let index = testIndex + 1; index < args.length; index += 1) {
    const value = args[index];
    if (consumesValue) {
      consumesValue = false;
      continue;
    }
    if (NODE_TEST_VALUE_OPTIONS.has(value)) {
      consumesValue = true;
      continue;
    }
    if (value.startsWith('-')) continue;
    files.push(index);
  }
  return files;
}

function stripNodeTestFiles(args: readonly string[]): string[] {
  const files = new Set(nodeTestFileIndexes(args));
  return args.filter((_value, index) => !files.has(index));
}

function workflowValue(
  spec: TestCommandSpec,
  runner: 'cargo' | 'node',
  currentStage: TestWorkflowStage,
  nextStage: TestWorkflowStage | undefined,
  nextArgs: string[] | undefined,
  workdir?: string
): JsonObject {
  const nextActions: JsonObject[] = [];
  if (nextStage && nextArgs) {
    const argumentsValue: JsonObject = { program: spec.program, args: nextArgs };
    if (workdir) argumentsValue.workdir = workdir;
    nextActions.push({
      action: 'run_next_test_stage',
      action_id: `test-stage-${nextStage}`,
      tool: 'exec_command',
      stage: nextStage,
      required_arguments: [],
      arguments: argumentsValue,
      reason: `test_${currentStage}_passed`
    });
  }
  return {
    schema_version: 1,
    runner,
    stage_sequence: ['preflight', 'focused', 'affected', 'full'],
    capability_preflight: 'automatic',
    scope_source: 'command',
    current_stage: currentStage,
    next_stage: nextStage ?? null,
    advance_condition: 'command_ok=true',
    full_regression_deferred: currentStage !== 'full',
    next_actions: nextActions
  };
}

export function testWorkflow(spec: TestCommandSpec, workdir?: string): JsonObject | undefined {
  const descriptor = describeTestRunner(spec);
  if (descriptor?.runner === 'cargo') {
    const filterIndex = cargoFilterIndex(spec.args);
    const targetScoped = cargoHasTargetScope(spec.args);
    if (filterIndex !== undefined) {
      const affectedArgs = spec.args.filter((_value, index) => index !== filterIndex);
      return workflowValue(
        spec,
        'cargo',
        'focused',
        targetScoped ? 'affected' : 'full',
        affectedArgs,
        workdir
      );
    }
    if (targetScoped) {
      return workflowValue(spec, 'cargo', 'affected', 'full', stripCargoTargetScope(spec.args), workdir);
    }
    return workflowValue(spec, 'cargo', 'full', undefined, undefined, workdir);
  }
  if (descriptor?.runner === 'node') {
    const focused = requestedSelector(spec.args)?.startsWith('--test-name-pattern') === true;
    if (focused) {
      const affectedArgs = removeOptionWithValue(spec.args, '--test-name-pattern');
      const affected = nodeTestFileIndexes(affectedArgs).length > 0;
      return workflowValue(spec, 'node', 'focused', affected ? 'affected' : 'full', affectedArgs, workdir);
    }
    if (nodeTestFileIndexes(spec.args).length > 0) {
      return workflowValue(spec, 'node', 'affected', 'full', stripNodeTestFiles(spec.args), workdir);
    }
    return workflowValue(spec, 'node', 'full', undefined, undefined, workdir);
  }
  return undefined;
}

function capabilitiesFromHelp(descriptor: TestRunnerDescriptor, output: string, probeOk: boolean): TestRunnerCapabilities {
  return {
    runner: descriptor.runner,
    probeOk,
    supportsFilter: output.includes('--filter'),
    supportsTestNamePattern: output.includes('--test-name-pattern'),
    selectorSyntax: descriptor.selectorSyntax
  };
}

function capabilityKey(spec: TestCommandSpec, descriptor: TestRunnerDescriptor, cwd: string): string {
  return `${cwd}\0${spec.program}\0${descriptor.helpArgs.join('\0')}`;
}

export function clearTestRunnerCapabilityCacheForTest(): void {
  capabilityCache.clear();
}

export async function preflightTestRunnerCapabilities(
  spec: TestCommandSpec,
  cwd: string,
  probe: TestRunnerCapabilityProbe
): Promise<JsonObject | undefined> {
  const descriptor = describeTestRunner(spec);
  const selector = descriptor ? requestedSelector(spec.args) : undefined;
  if (!descriptor || !selector) return undefined;

  const key = capabilityKey(spec, descriptor, cwd);
  const cached = capabilityCache.get(key);
  let capabilities = cached;
  let cacheStatus: 'hit' | 'miss' = cached ? 'hit' : 'miss';
  if (!capabilities) {
    try {
      const result = await probe(spec.program, descriptor.helpArgs, cwd);
      const output = `${result.stdout}\n${result.stderr}`;
      capabilities = capabilitiesFromHelp(descriptor, output, result.code === 0 || output.length > 0);
    } catch {
      capabilities = capabilitiesFromHelp(descriptor, '', false);
    }
    capabilityCache.set(key, capabilities);
  }

  const unsupported = capabilities.probeOk && (
    ((selector === '--filter' || selector.startsWith('--filter=')) && !capabilities.supportsFilter)
    || ((selector === '--test-name-pattern' || selector.startsWith('--test-name-pattern=')) && !capabilities.supportsTestNamePattern)
  );
  const metadata: JsonObject = {
    runner: capabilities.runner,
    capability_cache: cacheStatus,
    probe_ok: capabilities.probeOk,
    selector_syntax: capabilities.selectorSyntax,
    supports_filter: capabilities.supportsFilter,
    supports_test_name_pattern: capabilities.supportsTestNamePattern
  };
  if (!unsupported) return metadata;

  throw new ProcessToolError(
    'TEST_RUNNER_CAPABILITY_MISMATCH',
    `${capabilities.runner} does not support requested selector ${selector}`,
    'validation',
    false,
    {
      ...metadata,
      unsupported_argument: selector,
      stage: 'test_runner_preflight',
      suggestion: `Use ${capabilities.selectorSyntax} instead of ${selector}.`,
      recovery_actions: [{
        action: 'use_supported_test_selector',
        tool: 'exec_command',
        required_arguments: ['program', 'args'],
        reason: 'runner_selector_unsupported'
      }]
    }
  );
}
