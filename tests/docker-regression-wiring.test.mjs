import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

const read = relative => readFile(new URL(`../${relative}`, import.meta.url), 'utf8');

test('Docker regression exercises the dev deployment contract and is wired into CI', async () => {
  const [script, ci] = await Promise.all([
    read('scripts/test-node-agent-docker-regression.sh'),
    read('.github/workflows/ci.yml')
  ]);

  assert.match(script, /build dev/);
  assert.match(script, /Stop runtime and let dev take over the same port/);
  assert.match(script, /wait_container_http .*\/health/);
  assert.match(script, /wait_container_http .*\/ui\//);
  assert.match(script, /--restart-supervised bash/);
  assert.match(script, /docker exec -u node .*docker version/);
  assert.match(script, /cargo --version/);
  assert.match(script, /regression-marker/);
  assert.match(script, /dev data must not share the production \.data directory/);
  assert.match(script, /Restart dev and verify recovery/);
  assert.match(ci, /test-node-agent-docker-regression\.sh/);
});
