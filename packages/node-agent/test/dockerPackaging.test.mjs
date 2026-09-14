import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

const composePath = fileURLToPath(new URL('../../../docker-compose.yml', import.meta.url));
const dockerfilePath = fileURLToPath(new URL('../Dockerfile', import.meta.url));
const entrypointPath = fileURLToPath(new URL('../docker-entrypoint.sh', import.meta.url));

test('Docker Compose exposes the management UI only through host loopback while trusting the private bridge hop', async () => {
  const compose = await readFile(composePath, 'utf8');
  const dockerfile = await readFile(dockerfilePath, 'utf8');
  const entrypoint = await readFile(entrypointPath, 'utf8');

  assert.match(compose, /127\.0\.0\.1:\$\{CTMCP_PORT:-3789\}:3789/);
  assert.match(compose, /CTMCP_UI_ENABLED:\s*(?:true|"true"|'true'|1|"1"|'1')/);
  assert.match(compose, /CTMCP_UI_TRUST_PRIVATE_PROXY:\s*(?:true|"true"|'true'|1|"1"|'1')/);
  assert.doesNotMatch(dockerfile, /CTMCP_UI_TRUST_PRIVATE_PROXY/);
  assert.match(compose, /restart:\s*unless-stopped/);
  assert.match(entrypoint, /export CTMCP_RESTART_SUPERVISOR=active-v1/);
  assert.match(entrypoint, /node dist\/cli\.js --restart-supervised "\$@"/);
});
