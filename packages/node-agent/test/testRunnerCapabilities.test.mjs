import test from 'node:test';
import assert from 'node:assert/strict';
import {
  clearTestRunnerCapabilityCacheForTest,
  describeTestRunner,
  preflightTestRunnerCapabilities,
  testWorkflow
} from '../dist/processes/testRunnerCapabilities.js';

const codeception = {
  program: 'php',
  args: ['vendor/bin/codecept', 'run', 'unit', '--filter', 'Example']
};

test('recognizes Codeception through a PHP wrapper and probes command-specific help', () => {
  const descriptor = describeTestRunner(codeception);
  assert.equal(descriptor.runner, 'codeception');
  assert.deepEqual(descriptor.helpArgs, ['vendor/bin/codecept', 'run', '--help']);
  assert.equal(descriptor.selectorSyntax, 'tests/path/Test.php:testMethod');
});

test('unsupported Codeception --filter is rejected before execution and cached', async () => {
  clearTestRunnerCapabilityCacheForTest();
  let probes = 0;
  const probe = async (_program, args) => {
    probes += 1;
    assert.deepEqual(args, ['vendor/bin/codecept', 'run', '--help']);
    return { code: 0, stdout: 'Usage: codecept run [suite] [test]\n', stderr: '' };
  };

  await assert.rejects(
    () => preflightTestRunnerCapabilities(codeception, '/workspace', probe),
    error => {
      assert.equal(error.code, 'TEST_RUNNER_CAPABILITY_MISMATCH');
      assert.equal(error.details.capability_cache, 'miss');
      assert.equal(error.details.unsupported_argument, '--filter');
      assert.equal(error.details.selector_syntax, 'tests/path/Test.php:testMethod');
      return true;
    }
  );
  await assert.rejects(
    () => preflightTestRunnerCapabilities(codeception, '/workspace', probe),
    error => {
      assert.equal(error.code, 'TEST_RUNNER_CAPABILITY_MISMATCH');
      assert.equal(error.details.capability_cache, 'hit');
      return true;
    }
  );
  assert.equal(probes, 1);
});

test('supported selector returns capability metadata and cache hit', async () => {
  clearTestRunnerCapabilityCacheForTest();
  let probes = 0;
  const phpunit = { program: 'phpunit', args: ['--filter', 'Example'] };
  const probe = async () => {
    probes += 1;
    return { code: 0, stdout: 'Options:\n  --filter <pattern>\n', stderr: '' };
  };
  const first = await preflightTestRunnerCapabilities(phpunit, '/workspace', probe);
  const second = await preflightTestRunnerCapabilities(phpunit, '/workspace', probe);
  assert.equal(first.capability_cache, 'miss');
  assert.equal(first.supports_filter, true);
  assert.equal(second.capability_cache, 'hit');
  assert.equal(probes, 1);
});

test('runner command without a selector does not pay probe cost', async () => {
  clearTestRunnerCapabilityCacheForTest();
  let probes = 0;
  const result = await preflightTestRunnerCapabilities(
    { program: 'cargo', args: ['test', 'specific_test'] },
    '/workspace',
    async () => {
      probes += 1;
      return { code: 0, stdout: '', stderr: '' };
    }
  );
  assert.equal(result, undefined);
  assert.equal(probes, 0);
});

test('Node test workflow advances focused to affected to full with executable actions', () => {
  const focused = testWorkflow({
    program: 'node',
    args: ['--import', './test/setup.mjs', '--test', '--test-name-pattern', 'ambiguous', 'test/editRecovery.test.mjs']
  }, 'packages/node-agent');
  assert.equal(focused.current_stage, 'focused');
  assert.equal(focused.next_stage, 'affected');
  assert.equal(focused.advance_condition, 'command_ok=true');
  assert.deepEqual(focused.next_actions[0].required_arguments, []);
  assert.deepEqual(focused.next_actions[0].arguments, {
    program: 'node',
    args: ['--import', './test/setup.mjs', '--test', 'test/editRecovery.test.mjs'],
    workdir: 'packages/node-agent'
  });

  const affected = testWorkflow({
    program: 'node',
    args: ['--import', './test/setup.mjs', '--test', 'test/editRecovery.test.mjs']
  }, 'packages/node-agent');
  assert.equal(affected.current_stage, 'affected');
  assert.equal(affected.next_stage, 'full');
  assert.deepEqual(affected.next_actions[0].arguments.args, ['--import', './test/setup.mjs', '--test']);

  const full = testWorkflow({ program: 'node', args: ['--test'] });
  assert.equal(full.current_stage, 'full');
  assert.equal(full.next_stage, null);
  assert.deepEqual(full.next_actions, []);
});

test('Cargo test workflow preserves build scope while advancing focused to affected to full', () => {
  const focused = testWorkflow({
    program: 'cargo',
    args: ['test', '--manifest-path', 'src-tauri/Cargo.toml', '--lib', 'transport_failures_are_split_from_tool_failures']
  }, '.');
  assert.equal(focused.current_stage, 'focused');
  assert.equal(focused.next_stage, 'affected');
  assert.deepEqual(focused.next_actions[0].arguments.args, [
    'test', '--manifest-path', 'src-tauri/Cargo.toml', '--lib'
  ]);

  const affected = testWorkflow({
    program: 'cargo',
    args: ['test', '--manifest-path', 'src-tauri/Cargo.toml', '--lib']
  }, '.');
  assert.equal(affected.current_stage, 'affected');
  assert.equal(affected.next_stage, 'full');
  assert.deepEqual(affected.next_actions[0].arguments.args, [
    'test', '--manifest-path', 'src-tauri/Cargo.toml'
  ]);

  const full = testWorkflow({
    program: 'cargo',
    args: ['test', '--manifest-path', 'src-tauri/Cargo.toml']
  });
  assert.equal(full.current_stage, 'full');
  assert.deepEqual(full.next_actions, []);
});
