import assert from 'node:assert/strict';
import test from 'node:test';
import { chatConnectionTone, discussionConnectionTone } from '../src/lib/chat/connection-tone.ts';

test('work connection colors follow live transport and distinguish expired from deliberately detached', () => {
  assert.equal(chatConnectionTone(null), 'offline');
  assert.equal(chatConnectionTone({ status: 'offline', agent_name: 'old name' }), 'offline');
  for (const status of ['connected', 'waiting']) {
    assert.equal(chatConnectionTone({ status, work_state: 'processing' }), 'online');
  }
  assert.equal(chatConnectionTone({ status: 'offline', connection_id: 'public-id' }), 'stale');
  assert.equal(chatConnectionTone({ closed: true, status: 'connected', connection_id: 'public-id' }), 'offline');
});

test('legacy group attachment colors ignore paused members and stale work flags', () => {
  assert.equal(chatConnectionTone({ mode: 'group', status: 'offline', members: [] }), 'offline');
  assert.equal(chatConnectionTone({ mode: 'group', status: 'offline', members: [{ paused: true }] }), 'offline');
  assert.equal(chatConnectionTone({ mode: 'group', status: 'offline', members: [{ paused: false }], work_state: 'processing' }), 'stale');
});

test('discussion colors aggregate presence and respect explicit disconnect or archive', () => {
  assert.equal(discussionConnectionTone(undefined), 'offline');
  assert.equal(discussionConnectionTone({ member_details: [] }), 'offline');
  assert.equal(discussionConnectionTone({ member_details: [{ status: 'missing' }, { status: 'closed' }] }), 'offline');
  const stale = { member_details: [{ status: 'offline', busy: true }] };
  assert.equal(discussionConnectionTone(stale), 'stale');
  for (const status of ['connected', 'waiting']) {
    assert.equal(discussionConnectionTone({ member_details: [...stale.member_details, { status, error: true }] }), 'online');
  }
  assert.equal(discussionConnectionTone({ ...stale, paused: true }), 'offline');
  assert.equal(discussionConnectionTone({ archived: true, member_details: [{ status: 'connected' }] }), 'offline');
});
