import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { downloadedAdbPath, ensureAdb, PLATFORM_TOOLS, sha256Hex, verifyArchive } from './phone-adb.js';

const dirs: string[] = [];
const tmp = () => {
  const d = fs.mkdtempSync(path.join(os.tmpdir(), 'phone-adb-'));
  dirs.push(d);
  return d;
};
afterEach(() => dirs.splice(0).forEach((d) => fs.rmSync(d, { recursive: true, force: true })));

describe('platform-tools download', () => {
  it('pins official dl.google.com archives with sha256', () => {
    for (const a of Object.values(PLATFORM_TOOLS)) {
      expect(a.url).toMatch(/^https:\/\/dl\.google\.com\/android\/repository\/platform-tools_r[\d.]+-(darwin|linux|win)\.zip$/);
      expect(a.sha256).toMatch(/^[0-9a-f]{64}$/);
    }
  });

  it('refuses an archive whose checksum does not match', async () => {
    const dataDir = tmp();
    const saved = process.env.ALLTERNIT_ADB_PATH;
    const savedPath = process.env.PATH;
    delete process.env.ALLTERNIT_ADB_PATH;
    process.env.PATH = '';
    try {
      await expect(
        ensureAdb(dataDir, {
          platform: 'darwin',
          includeSystem: false,
          fetchImpl: (async () => new Response(Buffer.from('tampered'))) as unknown as typeof fetch,
          extract: async () => {
            throw new Error('must not extract');
          },
        }),
      ).rejects.toThrow(/checksum/);
    } finally {
      process.env.PATH = savedPath;
      if (saved) process.env.ALLTERNIT_ADB_PATH = saved;
    }
  });

  it('verifies a matching archive and unpacks it', async () => {
    const dataDir = tmp();
    const body = Buffer.from('fake zip');
    const real = PLATFORM_TOOLS.linux;
    PLATFORM_TOOLS.linux = { ...real, sha256: sha256Hex(body), size: body.length };
    const savedPath = process.env.PATH;
    const saved = process.env.ALLTERNIT_ADB_PATH;
    delete process.env.ALLTERNIT_ADB_PATH;
    process.env.PATH = '';
    try {
      expect(verifyArchive(body, PLATFORM_TOOLS.linux)).toBe(true);
      const adb = await ensureAdb(dataDir, {
        platform: 'linux',
        includeSystem: false,
        fetchImpl: (async () => new Response(body)) as unknown as typeof fetch,
        extract: async (_zip, dest) => {
          const target = path.join(dest, 'platform-tools', 'adb');
          fs.mkdirSync(path.dirname(target), { recursive: true });
          fs.writeFileSync(target, '#!/bin/sh\n');
        },
      });
      expect(adb).toBe(downloadedAdbPath(dataDir, 'linux'));
      expect(fs.existsSync(adb)).toBe(true);
    } finally {
      PLATFORM_TOOLS.linux = real;
      process.env.PATH = savedPath;
      if (saved) process.env.ALLTERNIT_ADB_PATH = saved;
    }
  });
});
