import {
  createCipheriv, createDecipheriv, createHash, createPrivateKey, generateKeyPairSync,
  randomBytes, randomUUID, type KeyObject
} from 'node:crypto';
import { mkdir, readFile, rename, writeFile } from 'node:fs/promises';
import { hostname } from 'node:os';
import path from 'node:path';
import type { AgentConfig } from '../types.js';
import type { TunnelEndpoint } from './endpoint.js';

export interface DeviceIdentity {
  deviceId: string;
  clientId: string;
  privateKeyDer: string;
  publicKeyRaw: string;
  enrolled: boolean;
  serverId?: string;
  serverPublicKeyRaw?: string;
}

interface EncryptedIdentity {
  version: 1;
  iv: string;
  tag: string;
  ciphertext: string;
}

function encryptionKey(secret: string): Buffer {
  return createHash('sha256').update(secret).digest();
}

function encryptIdentity(identity: DeviceIdentity, secret: string): EncryptedIdentity {
  const iv = randomBytes(12);
  const cipher = createCipheriv('aes-256-gcm', encryptionKey(secret), iv);
  const ciphertext = Buffer.concat([cipher.update(JSON.stringify(identity), 'utf8'), cipher.final()]);
  return { version: 1, iv: iv.toString('base64url'), tag: cipher.getAuthTag().toString('base64url'), ciphertext: ciphertext.toString('base64url') };
}

function decryptIdentity(envelope: EncryptedIdentity, secret: string): DeviceIdentity {
  if (envelope.version !== 1) throw new Error('unsupported tunnel identity version');
  const decipher = createDecipheriv('aes-256-gcm', encryptionKey(secret), Buffer.from(envelope.iv, 'base64url'));
  decipher.setAuthTag(Buffer.from(envelope.tag, 'base64url'));
  const plaintext = Buffer.concat([decipher.update(Buffer.from(envelope.ciphertext, 'base64url')), decipher.final()]);
  return JSON.parse(plaintext.toString('utf8')) as DeviceIdentity;
}

async function saveIdentity(file: string, identity: DeviceIdentity, secret: string): Promise<void> {
  await mkdir(path.dirname(file), { recursive: true });
  const temporary = `${file}.${process.pid}.tmp`;
  await writeFile(temporary, `${JSON.stringify(encryptIdentity(identity, secret), null, 2)}\n`, { mode: 0o600 });
  await rename(temporary, file);
}

async function readIdentity(file: string, secret: string): Promise<DeviceIdentity | undefined> {
  try {
    return decryptIdentity(JSON.parse(await readFile(file, 'utf8')) as EncryptedIdentity, secret);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return undefined;
    throw error;
  }
}

function newIdentity(clientId: string): DeviceIdentity {
  const pair = generateKeyPairSync('ed25519');
  const privateKeyDer = pair.privateKey.export({ format: 'der', type: 'pkcs8' }).toString('base64');
  const publicDer = pair.publicKey.export({ format: 'der', type: 'spki' });
  const publicKeyRaw = publicDer.subarray(publicDer.length - 32).toString('base64url');
  return { deviceId: randomUUID().replaceAll('-', ''), clientId, privateKeyDer, publicKeyRaw, enrolled: false };
}

export function identityPrivateKey(identity: DeviceIdentity): KeyObject {
  return createPrivateKey({ key: Buffer.from(identity.privateKeyDer, 'base64'), format: 'der', type: 'pkcs8' });
}

function expectedServerId(publicKeyRaw: string): string | undefined {
  if (!/^[A-Za-z0-9_-]{43}$/.test(publicKeyRaw)) return undefined;
  try {
    const raw = Buffer.from(publicKeyRaw, 'base64url');
    if (raw.length !== 32) return undefined;
    return createHash('sha256').update(raw).digest('base64url');
  } catch {
    return undefined;
  }
}

export function hasPinnedServerIdentity(identity: DeviceIdentity): boolean {
  if (!identity.serverId || !/^[A-Za-z0-9_-]{43}$/.test(identity.serverId)) return false;
  if (!identity.serverPublicKeyRaw) return false;
  return expectedServerId(identity.serverPublicKeyRaw) === identity.serverId;
}

function validateEnrollmentUrl(publicUrl: string, value: string): URL {
  const publicEndpoint = new URL(publicUrl);
  const enrollment = new URL(value.trim());
  const sameOrigin = enrollment.protocol === 'https:'
    && enrollment.hostname === publicEndpoint.hostname
    && enrollment.port === publicEndpoint.port;
  if (!sameOrigin || enrollment.username || enrollment.password || enrollment.search || enrollment.hash) {
    throw new Error('enrollment URL must use the same HTTPS origin as the built-in tunnel');
  }
  if (!/^\/_tunnel\/enroll\/[A-Za-z0-9]{1,128}$/.test(enrollment.pathname)) {
    throw new Error('enrollment URL must use /_tunnel/enroll/<code>');
  }
  return enrollment;
}

export async function loadOrEnroll(
  config: AgentConfig,
  endpoint: TunnelEndpoint,
  enrollmentFetch: typeof fetch
): Promise<{ identity: DeviceIdentity; enrollmentCompleted: boolean }> {
  if (!config.tunnel) throw new Error('built-in tunnel is not configured');
  const enrollmentUrl = config.tunnel.enrollmentUrl?.trim();
  const stored = await readIdentity(config.tunnel.stateFile, config.oauth.tokenSecret);
  if (stored?.enrolled && hasPinnedServerIdentity(stored)) {
    return { identity: stored, enrollmentCompleted: false };
  }

  let identity = stored ?? newIdentity(endpoint.clientId);
  if (!enrollmentUrl) {
    await saveIdentity(config.tunnel.stateFile, identity, config.oauth.tokenSecret);
    if (identity.enrolled && !hasPinnedServerIdentity(identity)) {
      throw new Error('built-in tunnel v4 requires re-enrollment to pin the server identity');
    }
    throw new Error('built-in tunnel enrollment URL is required for the first connection');
  }
  const enrollment = validateEnrollmentUrl(endpoint.publicUrl, enrollmentUrl);
  const response = await enrollmentFetch(enrollment, {
    method: 'POST', redirect: 'manual', headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ device_id: identity.deviceId, client_id: identity.clientId, device_name: hostname() || 'Coding Tools MCP Node', public_key: identity.publicKeyRaw })
  });
  if (!response.ok) throw new Error(`built-in tunnel enrollment failed (${response.status}): ${(await response.text()).trim()}`);
  const enrolled = await response.json() as {
    device_id?: string;
    client_id?: string;
    server_id?: string;
    server_public_key?: string;
  };
  if (enrolled.device_id !== identity.deviceId) throw new Error('built-in tunnel enrollment returned a different device ID');
  const clientId = String(enrolled.client_id || identity.clientId).trim();
  if (!/^[A-Za-z0-9_-]{1,64}$/.test(clientId)) throw new Error('built-in tunnel enrollment returned an invalid client ID');
  const serverId = String(enrolled.server_id || '').trim();
  const serverPublicKeyRaw = String(enrolled.server_public_key || '').trim();
  if (!/^[A-Za-z0-9_-]{43}$/.test(serverId) || expectedServerId(serverPublicKeyRaw) !== serverId) {
    throw new Error('built-in tunnel enrollment returned an invalid server identity');
  }
  identity = { ...identity, clientId, enrolled: true, serverId, serverPublicKeyRaw };
  await saveIdentity(config.tunnel.stateFile, identity, config.oauth.tokenSecret);
  return { identity, enrollmentCompleted: true };
}
