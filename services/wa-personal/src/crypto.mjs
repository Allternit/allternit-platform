// Encrypted-at-rest session storage: AES-256-GCM, one file, atomic writes, 0600.
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

const MAGIC = Buffer.from('WAP1');

/** 32-byte key from ALLTERNIT_WA_PERSONAL_KEY (hex or base64). Throws if malformed. */
export function parseKey(raw) {
  const s = String(raw).trim();
  const buf = /^[0-9a-fA-F]{64}$/.test(s) ? Buffer.from(s, 'hex') : Buffer.from(s, 'base64');
  if (buf.length !== 32) throw new Error('ALLTERNIT_WA_PERSONAL_KEY must be 32 bytes (64 hex chars or base64)');
  return buf;
}

/**
 * The key comes from the env (the runtime's secret store hands it in). Without one
 * we create a random key file outside the session directory (0600), so session
 * files alone are never enough to link the number.
 */
export function loadKey({ env = process.env, keyFile }) {
  if (env.ALLTERNIT_WA_PERSONAL_KEY) return parseKey(env.ALLTERNIT_WA_PERSONAL_KEY);
  try {
    return parseKey(fs.readFileSync(keyFile, 'utf8'));
  } catch (e) {
    if (e.code !== 'ENOENT') throw e;
  }
  fs.mkdirSync(path.dirname(keyFile), { recursive: true, mode: 0o700 });
  const key = crypto.randomBytes(32);
  fs.writeFileSync(keyFile, key.toString('hex'), { mode: 0o600, flag: 'wx' });
  return key;
}

export function seal(key, plain) {
  const iv = crypto.randomBytes(12);
  const c = crypto.createCipheriv('aes-256-gcm', key, iv);
  const ct = Buffer.concat([c.update(plain, 'utf8'), c.final()]);
  return Buffer.concat([MAGIC, iv, c.getAuthTag(), ct]);
}

export function open(key, blob) {
  if (blob.length < 4 + 12 + 16 || !blob.subarray(0, 4).equals(MAGIC)) throw new Error('not a wa-personal session file');
  const d = crypto.createDecipheriv('aes-256-gcm', key, blob.subarray(4, 16));
  d.setAuthTag(blob.subarray(16, 32));
  return Buffer.concat([d.update(blob.subarray(32)), d.final()]).toString('utf8');
}

export function writeSealed(file, key, plain) {
  fs.mkdirSync(path.dirname(file), { recursive: true, mode: 0o700 });
  const tmp = `${file}.${process.pid}.tmp`;
  fs.writeFileSync(tmp, seal(key, plain), { mode: 0o600 });
  fs.renameSync(tmp, file);
}

/** Returns null when the file is missing; throws when it can't be decrypted (wrong key / tampered). */
export function readSealed(file, key) {
  let blob;
  try {
    blob = fs.readFileSync(file);
  } catch (e) {
    if (e.code === 'ENOENT') return null;
    throw e;
  }
  return open(key, blob);
}
