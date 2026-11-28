import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { afterEach, describe, expect, it } from 'vitest';
import { KERNEL_KEY_SECRET, kernelSeedLine, loadOrCreateKernelSeed, type KernelKeyStore } from './kernel-key.js';

const SEED = 'ab'.repeat(32);

function memoryStore(initial: Record<string, string> = {}): KernelKeyStore & { data: Record<string, string> } {
  const data = { ...initial };
  return { data, read: (k) => data[k] ?? null, write: (k, v) => { data[k] = v; } };
}

const dirs: string[] = [];
function legacyFile(contents?: string): string {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'kernel-key-'));
  dirs.push(dir);
  const file = path.join(dir, 'kernel', 'authority-ed25519.key');
  if (contents !== undefined) {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, contents, { mode: 0o600 });
  }
  return file;
}

afterEach(() => {
  for (const d of dirs.splice(0)) fs.rmSync(d, { recursive: true, force: true });
});

describe('kernel signing key in the OS keystore (CONTRACT.md 12.3)', () => {
  it('imports the old 0600 file into the keystore and deletes the file', () => {
    const store = memoryStore();
    const file = legacyFile(`${SEED}\n`);
    const result = loadOrCreateKernelSeed(store, file);
    expect(result).toEqual({ seedHex: SEED, source: 'imported-file' });
    expect(store.data[KERNEL_KEY_SECRET]).toBe(SEED);
    expect(fs.existsSync(file)).toBe(false);
  });

  it('first run generates a seed in the keystore and writes no file', () => {
    const store = memoryStore();
    const file = legacyFile();
    const result = loadOrCreateKernelSeed(store, file);
    expect(result.source).toBe('generated');
    expect(result.seedHex).toMatch(/^[0-9a-f]{64}$/);
    expect(store.data[KERNEL_KEY_SECRET]).toBe(result.seedHex);
    expect(fs.existsSync(file)).toBe(false);
  });

  it('keeps the file and uses it this launch when the keystore refuses the import', () => {
    const file = legacyFile(SEED);
    const locked: KernelKeyStore = {
      read: () => null,
      write: () => { throw new Error('Keychain unavailable: denied'); },
    };
    expect(loadOrCreateKernelSeed(locked, file)).toEqual({ seedHex: SEED, source: 'file-fallback' });
    expect(fs.existsSync(file)).toBe(true);
  });

  it('a locked keystore with no file is an error, never a new file', () => {
    const file = legacyFile();
    const locked: KernelKeyStore = { read: () => { throw new Error('Keychain unavailable: pending'); }, write: () => {} };
    expect(() => loadOrCreateKernelSeed(locked, file)).toThrow(/Keychain/);
    expect(fs.existsSync(file)).toBe(false);
  });

  it('leaves a different key file alone', () => {
    const other = 'cd'.repeat(32);
    const file = legacyFile(other);
    const result = loadOrCreateKernelSeed(memoryStore({ [KERNEL_KEY_SECRET]: SEED }), file);
    expect(result).toEqual({ seedHex: SEED, source: 'keystore' });
    expect(fs.readFileSync(file, 'utf8')).toBe(other);
  });

  it('frames the stdin line, and sends an empty line when there is no seed', () => {
    expect(kernelSeedLine(SEED)).toBe(`kernel-seed:${SEED}\n`);
    expect(kernelSeedLine(null)).toBe('\n');
    expect(kernelSeedLine('not-hex')).toBe('\n');
  });
});
