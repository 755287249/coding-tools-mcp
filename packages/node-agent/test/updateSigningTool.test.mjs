import assert from 'node:assert/strict';
import { generateKeyPairSync, verify } from 'node:crypto';
import test from 'node:test';
import { signUpdateMetadata, signingPayload } from '../../../scripts/sign-node-agent-update.mjs';
import { verifyNodeUpdateArtifact } from '../dist/liveUpdate.js';

function fixture() {
  const { privateKey, publicKey } = generateKeyPairSync('ed25519');
  return {
    artifact: Buffer.from('portable-update-fixture'),
    privateKeyPem: privateKey.export({ format: 'pem', type: 'pkcs8' }),
    publicKey
  };
}

test('offline signing tool emits metadata accepted by Node Agent live-update verification', () => {
  const { artifact, privateKeyPem } = fixture();
  const metadata = signUpdateMetadata({
    artifact,
    privateKeyPem,
    version: '0.29.34',
    url: 'https://updates.example.test/ctnode.zip'
  });
  const offer = {
    update_id: 'release-02934',
    version: metadata.version,
    url: metadata.url,
    sha256: metadata.sha256,
    signature: metadata.signature
  };
  assert.deepEqual(verifyNodeUpdateArtifact(offer, artifact, metadata.trustedPublicKey), offer);
});

test('offline signing tool signs the domain-separated version and SHA-256 payload', () => {
  const { artifact, privateKeyPem, publicKey } = fixture();
  const metadata = signUpdateMetadata({
    artifact,
    privateKeyPem,
    version: '0.29.34',
    url: 'https://updates.example.test/ctnode.zip'
  });
  assert.equal(
    verify(null, signingPayload(metadata.version, metadata.sha256), publicKey, Buffer.from(metadata.signature, 'base64url')),
    true
  );
  assert.throws(() => signUpdateMetadata({
    artifact,
    privateKeyPem,
    version: '0.29.34',
    url: 'http://updates.example.test/ctnode.zip'
  }), /HTTPS/);
});
