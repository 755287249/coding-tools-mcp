import test from 'node:test';
import assert from 'node:assert/strict';
import { generateKeyPairSync, sign } from 'node:crypto';
import { mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';

import {
  NODE_UPDATE_PUBLIC_KEY_ENV,
  assertSafeArchiveEntries,
  downloadVerifiedNodeUpdateArchive,
  nodeUpdateSigningPayload,
  normalizeNodeUpdateOffer,
  sha256Hex,
  trustedNodeUpdatePublicKey,
  scheduleNodeUpdateHandoff,
  verifyNodeUpdateArtifact,
  verifyNodeUpdateOfferSignature,
  verifyPortableNodePackage
} from '../dist/liveUpdate.js';

function fixture(bytes = Buffer.from('signed portable archive')) {
  const { publicKey, privateKey } = generateKeyPairSync('ed25519');
  const der = publicKey.export({ format: 'der', type: 'spki' });
  const publicKeyRaw = Buffer.from(der).subarray(-32).toString('base64url');
  const sha256 = sha256Hex(bytes);
  const version = '0.29.34';
  const signature = sign(null, nodeUpdateSigningPayload(version, sha256), privateKey).toString('base64url');
  return {
    bytes,
    publicKeyRaw,
    offer: {
      update_id: 'update-1',
      version,
      url: 'https://updates.example.test/ctnode.zip',
      sha256,
      signature
    }
  };
}

test('normalizes fixed-schema Node update offers and requires HTTPS', () => {
  const { offer } = fixture();
  assert.deepEqual(normalizeNodeUpdateOffer(offer), offer);
  assert.throws(() => normalizeNodeUpdateOffer({ ...offer, url: 'http://updates.example.test/ctnode.zip' }), /must use HTTPS/);
  assert.throws(() => normalizeNodeUpdateOffer({ ...offer, sha256: 'bad' }), /64 hexadecimal/);
  assert.throws(() => normalizeNodeUpdateOffer({ ...offer, signature: 'short' }), /canonical 64-byte/);
});

test('requires a locally trusted raw Ed25519 update public key', () => {
  const previous = process.env[NODE_UPDATE_PUBLIC_KEY_ENV];
  delete process.env[NODE_UPDATE_PUBLIC_KEY_ENV];
  try {
    assert.throws(() => trustedNodeUpdatePublicKey(), /is required/);
    assert.throws(() => trustedNodeUpdatePublicKey('short'), /canonical raw 32-byte/);
  } finally {
    if (previous === undefined) delete process.env[NODE_UPDATE_PUBLIC_KEY_ENV];
    else process.env[NODE_UPDATE_PUBLIC_KEY_ENV] = previous;
  }
});

test('accepts only offers signed by the locally trusted release key', () => {
  const { offer, publicKeyRaw } = fixture();
  assert.equal(verifyNodeUpdateOfferSignature(offer, publicKeyRaw).version, offer.version);
  assert.throws(() => verifyNodeUpdateOfferSignature({ ...offer, version: '0.29.35' }, publicKeyRaw), /signature verification failed/);

  const other = fixture(Buffer.from('different archive'));
  assert.throws(() => verifyNodeUpdateOfferSignature(offer, other.publicKeyRaw), /signature verification failed/);
});

test('verifies artifact SHA-256 only after the offer signature is valid', () => {
  const { offer, publicKeyRaw, bytes } = fixture();
  assert.equal(verifyNodeUpdateArtifact(offer, bytes, publicKeyRaw).sha256, offer.sha256);
  assert.throws(() => verifyNodeUpdateArtifact(offer, Buffer.from('tampered'), publicKeyRaw), /SHA-256 mismatch/);
});

test('downloads only a signed artifact whose bytes match the signed SHA-256', async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'ctmcp-live-update-'));
  const destination = path.join(directory, 'update.zip');
  const { offer, publicKeyRaw, bytes } = fixture();
  const fetchImpl = async () => new Response(bytes, {
    status: 200,
    headers: { 'content-length': String(bytes.length) }
  });
  await downloadVerifiedNodeUpdateArchive(offer, destination, { trustedKeyRaw: publicKeyRaw, fetchImpl });
  assert.deepEqual(await readFile(destination), bytes);

  const tampered = async () => new Response(Buffer.from('tampered'), { status: 200 });
  await assert.rejects(
    downloadVerifiedNodeUpdateArchive(offer, path.join(directory, 'tampered.zip'), { trustedKeyRaw: publicKeyRaw, fetchImpl: tampered }),
    /SHA-256 mismatch/
  );
});

test('rejects archive entries that could escape the staging directory', () => {
  assert.doesNotThrow(() => assertSafeArchiveEntries(['app/dist/cli.js', 'portable-manifest.json']));
  assert.throws(() => assertSafeArchiveEntries(['../outside.exe']), /path traversal/);
  assert.throws(() => assertSafeArchiveEntries(['C:/outside.exe']), /absolute path/);
  assert.throws(() => assertSafeArchiveEntries(['/outside.exe']), /absolute path/);
});

test('schedules whole-portable handoff with the current package as rollback fallback', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'ctmcp-current-portable-'));
  const dataDir = await mkdtemp(path.join(os.tmpdir(), 'ctmcp-data-'));
  await writeFile(path.join(root, 'update-handoff.ps1'), 'param()');
  const calls = [];
  await scheduleNodeUpdateHandoff({
    offer: fixture().offer,
    archivePath: path.join(dataDir, 'update.zip'),
    packageRoot: path.join(dataDir, 'package'),
    gitCommit: 'a'.repeat(40)
  }, dataDir, {
    currentPackageRoot: root,
    commandRunner: async (program, args) => {
      calls.push({ program, args: [...args] });
      return { code: 0, stdout: JSON.stringify({ ok: true, handoffScheduled: true }), stderr: '' };
    }
  });
  assert.equal(calls.length, 1);
  assert.equal(calls[0].program, 'powershell.exe');
  assert.ok(calls[0].args.includes('-FallbackPackageRoot'));
  assert.equal(calls[0].args[calls[0].args.indexOf('-FallbackPackageRoot') + 1], path.resolve(root));
});

test('verifies the staged portable manifest and every critical internal checksum', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'ctmcp-portable-'));
  const files = new Map([
    ['start-node-agent.bat', Buffer.from('@echo off\r\n')],
    ['portable-manifest.json', Buffer.from(JSON.stringify({ nodeAgentVersion: '0.29.34', gitCommit: 'a'.repeat(40), nodeRuntimeBundled: false }))],
    ['app/dist/cli.js', Buffer.from('cli')],
    ['app/dist/server.js', Buffer.from('server')],
    ['app/dist/ctmcp-protect.exe', Buffer.from('protect')],
    ['update-handoff.ps1', Buffer.from('handoff')]
  ]);
  for (const [relative, bytes] of files) {
    const target = path.join(root, ...relative.split('/'));
    await mkdir(path.dirname(target), { recursive: true });
    await writeFile(target, bytes);
  }
  const sums = [...files].map(([relative, bytes]) => `${sha256Hex(bytes)}  ${relative}`).join('\n') + '\n';
  await writeFile(path.join(root, 'SHA256SUMS.txt'), sums);
  assert.deepEqual(await verifyPortableNodePackage(root, '0.29.34'), { gitCommit: 'a'.repeat(40) });

  await writeFile(path.join(root, 'app/dist/server.js'), 'tampered');
  await assert.rejects(verifyPortableNodePackage(root, '0.29.34'), /critical file checksum mismatch/);
});
