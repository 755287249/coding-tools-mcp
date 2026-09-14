#!/usr/bin/env node
import { createHash, createPrivateKey, createPublicKey, sign } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';

export const NODE_UPDATE_SIGNATURE_DOMAIN = 'coding-tools-node-update-v1';

export function signingPayload(version, sha256) {
  return Buffer.from(`${NODE_UPDATE_SIGNATURE_DOMAIN}\n${version}\n${sha256.toLowerCase()}\n`, 'utf8');
}

export function signUpdateMetadata({ artifact, version, url, privateKeyPem }) {
  if (!Buffer.isBuffer(artifact)) throw new TypeError('artifact must be a Buffer');
  if (!/^[0-9A-Za-z.+-]{1,64}$/.test(version)) throw new Error('version contains unsupported characters');
  const parsed = new URL(url);
  if (parsed.protocol !== 'https:' || parsed.username || parsed.password) {
    throw new Error('url must use HTTPS without embedded credentials');
  }
  const privateKey = createPrivateKey(privateKeyPem);
  if (privateKey.asymmetricKeyType !== 'ed25519') throw new Error('private key must be Ed25519');
  const sha256 = createHash('sha256').update(artifact).digest('hex');
  const signature = sign(null, signingPayload(version, sha256), privateKey).toString('base64url');
  const publicDer = createPublicKey(privateKey).export({ format: 'der', type: 'spki' });
  const publicKeyRaw = Buffer.from(publicDer).subarray(-32).toString('base64url');
  return {
    version,
    url: parsed.toString(),
    sha256,
    signature,
    trustedPublicKey: publicKeyRaw
  };
}

function parseArgs(argv) {
  const values = new Map();
  for (let index = 0; index < argv.length; index += 1) {
    const name = argv[index];
    if (!name.startsWith('--')) throw new Error(`unexpected argument: ${name}`);
    const value = argv[index + 1];
    if (!value || value.startsWith('--')) throw new Error(`missing value for ${name}`);
    values.set(name.slice(2), value);
    index += 1;
  }
  for (const required of ['artifact', 'version', 'url', 'private-key']) {
    if (!values.has(required)) throw new Error(`--${required} is required`);
  }
  return values;
}

export async function main(argv = process.argv.slice(2)) {
  const args = parseArgs(argv);
  const [artifact, privateKeyPem] = await Promise.all([
    readFile(args.get('artifact')),
    readFile(args.get('private-key'), 'utf8')
  ]);
  const metadata = signUpdateMetadata({
    artifact,
    version: args.get('version'),
    url: args.get('url'),
    privateKeyPem
  });
  process.stdout.write(`${JSON.stringify(metadata, null, 2)}\n`);
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) {
  main().catch(error => {
    process.stderr.write(`sign-node-agent-update: ${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  });
}
