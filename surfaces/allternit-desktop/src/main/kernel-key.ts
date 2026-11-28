/**
 * The AllternitOS authority kernel's Ed25519 signing seed on Desktop
 * (CONTRACT.md section 12.3, G0 answer 3).
 *
 * allternit-api signs approval tokens and receipts with this key. On Desktop
 * the model's shell runs as the same OS user, so the seed must not sit in a
 * file a same-user process can read. Electron main keeps it in the OS keystore
 * through secure-store (macOS Keychain, Windows DPAPI via safeStorage, Linux
 * Secret Service via safeStorage) and writes it to allternit-api's stdin at
 * launch as `kernel-seed:<64 hex>`, after the D16 human-proof line
 * (backend-manager.ts). allternit-api then never reads or creates the file
 * (`cmd/allternit-api/src/authority/keystore.rs`).
 *
 * First run: a new seed is generated and stored in the keystore only.
 * Upgrade: an old 0600 file (`<data dir>/kernel/authority-ed25519.key`, or
 * `$ALLTERNIT_KERNEL_SIGNING_KEY`) is imported into the keystore, read back,
 * and only then deleted. If the keystore can't take it, the file is left in
 * place and used this launch, so the key is never lost.
 *
 * Key continuity: once a seed has been stored, a non-secret marker file
 * (`<data dir>/kernel/seed-stored`) says so. If the keystore entry later reads
 * back as missing or damaged while the marker exists, no new seed is generated:
 * a new key would silently orphan the signature on every old receipt. The
 * launch fails closed (KernelKeyLostError, no seed handed over) until the
 * entry is restored.
 *
 * The seed is never logged. Development builds use secure-store's local
 * envelope instead of the Keychain (same as every other Desktop secret), so a
 * rebuild does not prompt; packaged builds always use the OS keystore.
 */
import * as crypto from 'crypto';
import * as fs from 'fs';
import * as path from 'path';

export const KERNEL_KEY_SECRET = 'kernel-authority-ed25519';
export const KERNEL_KEY_STDIN_ENV = 'ALLTERNIT_KERNEL_KEY_STDIN';
export const KERNEL_SIGNING_KEY_ENV = 'ALLTERNIT_KERNEL_SIGNING_KEY';
const SEED_PREFIX = 'kernel-seed:';
const SEED_HEX = /^[0-9a-f]{64}$/;

export type KernelKeyStore = {
  read(key: string): string | null;
  write(key: string, value: string): void;
};

/** A seed was stored before but the keystore no longer yields it. Never answered by generating a new one. */
export class KernelKeyLostError extends Error {
  constructor() {
    super('the kernel signing key was stored before but cannot be read now (keystore entry missing or damaged); not generating a new key because old receipts would stop verifying');
    this.name = 'KernelKeyLostError';
  }
}

/** Non-secret marker beside the legacy key path: "a seed has been stored on this machine". */
export function kernelKeyMarkerPath(dataDir: string): string {
  return path.join(dataDir, 'kernel', 'seed-stored');
}

function seedWasStored(markerFile: string | undefined): boolean {
  return markerFile !== undefined && fs.existsSync(markerFile);
}

function markSeedStored(markerFile: string | undefined): void {
  if (markerFile === undefined) return;
  try {
    fs.mkdirSync(path.dirname(markerFile), { recursive: true });
    // No key material in the marker, only the fact that a seed was stored.
    fs.writeFileSync(markerFile, `stored ${new Date().toISOString()}\n`, { mode: 0o600 });
  } catch {
    /* the marker is a safety net, not a requirement */
  }
}

export type KernelKeyResult = {
  seedHex: string;
  /** Where it came from this launch, for one log line (never the seed). */
  source: 'keystore' | 'imported-file' | 'generated' | 'file-fallback';
};

/** The file allternit-api used before the keystore move. */
export function legacyKernelKeyPath(dataDir: string, env: NodeJS.ProcessEnv = process.env): string {
  const override = env[KERNEL_SIGNING_KEY_ENV];
  return override && override.length > 0 ? override : path.join(dataDir, 'kernel', 'authority-ed25519.key');
}

function readLegacySeed(file: string): string | null {
  try {
    const text = fs.readFileSync(file, 'utf8').trim().toLowerCase();
    return SEED_HEX.test(text) ? text : null;
  } catch {
    return null;
  }
}

/**
 * Load the seed from the keystore, importing or generating it as needed.
 * Errors from the store (a locked or denied Keychain) propagate unless an old
 * file can stand in for this launch: the caller then hands no seed over, and
 * kernel approvals stay unavailable (fail closed) instead of falling back to a
 * new file.
 */
export function loadOrCreateKernelSeed(store: KernelKeyStore, legacyFile: string, markerFile?: string): KernelKeyResult {
  const result = loadSeed(store, legacyFile, markerFile);
  markSeedStored(markerFile);
  return result;
}

function loadSeed(store: KernelKeyStore, legacyFile: string, markerFile: string | undefined): KernelKeyResult {
  const legacy = readLegacySeed(legacyFile);
  let stored: string | null;
  try {
    stored = store.read(KERNEL_KEY_SECRET);
  } catch (error) {
    if (legacy) return { seedHex: legacy, source: 'file-fallback' };
    throw error;
  }
  if (stored && SEED_HEX.test(stored)) {
    // A leftover copy of the same key: remove it. A different one is not ours to delete.
    if (legacy === stored) fs.rmSync(legacyFile, { force: true });
    return { seedHex: stored, source: 'keystore' };
  }
  // A seed was stored before and the entry is gone or damaged: do not mint a new one.
  if (!legacy && seedWasStored(markerFile)) throw new KernelKeyLostError();
  const seedHex = legacy ?? crypto.randomBytes(32).toString('hex');
  try {
    store.write(KERNEL_KEY_SECRET, seedHex);
    if (store.read(KERNEL_KEY_SECRET) !== seedHex) throw new Error('kernel key did not read back from the keystore');
  } catch (error) {
    if (legacy) return { seedHex: legacy, source: 'file-fallback' };
    throw error;
  }
  if (legacy) {
    fs.rmSync(legacyFile, { force: true });
    return { seedHex, source: 'imported-file' };
  }
  return { seedHex, source: 'generated' };
}

/** The stdin line allternit-api reads after the human-proof line. Empty seed = none handed over. */
export function kernelSeedLine(seedHex: string | null): string {
  return seedHex && SEED_HEX.test(seedHex) ? `${SEED_PREFIX}${seedHex}\n` : '\n';
}
