import test from 'node:test';
import assert from 'node:assert/strict';
import {
  MAX_TOOL_EVOLUTION_PROPOSAL_IDS,
  normalizeToolEvolutionProposalIds,
  toolEvolutionProposalIdsFromCommitMessage,
  toolEvolutionProposalIdsFromEnvironment
} from '../scripts/build-info-lib.mjs';

const id = character => character.repeat(64);

test('build info extracts only exact bounded Tool Evolution proposal trailers', () => {
  const message = [
    'feat: candidate implementation',
    '',
    `Tool-Evolution-Proposal: ${id('A')}`,
    `Tool-Evolution-Proposal: ${id('b')}`,
    `Tool-Evolution-Proposal: ${id('A')}`,
    'Tool-Evolution-Proposal: not-a-proposal-id',
    `prefix Tool-Evolution-Proposal: ${id('c')}`,
    ''
  ].join('\n');
  assert.deepEqual(toolEvolutionProposalIdsFromCommitMessage(message), [id('a'), id('b')]);
});

test('build info environment proposal claims are normalized, deduplicated, and capped', () => {
  const values = Array.from({ length: MAX_TOOL_EVOLUTION_PROPOSAL_IDS + 5 }, (_, index) =>
    index.toString(16).padStart(64, '0')
  );
  const parsed = toolEvolutionProposalIdsFromEnvironment(`${values.join(',')} invalid ${values[0].toUpperCase()}`);
  assert.equal(parsed.length, MAX_TOOL_EVOLUTION_PROPOSAL_IDS);
  assert.deepEqual(parsed, [...parsed].sort());
  assert.equal(parsed.filter(value => value === values[0]).length, 1);
  assert.deepEqual(normalizeToolEvolutionProposalIds(['bad', id('F')]), [id('f')]);
});
