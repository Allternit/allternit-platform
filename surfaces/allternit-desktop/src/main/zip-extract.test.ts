import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import * as zlib from 'node:zlib';
import { describe, expect, it } from 'vitest';
import { extractZipSafely, unsafeZipEntryName } from './zip-extract.js';

function crc32(b: Buffer): number {
  let crc = 0xffffffff;
  for (const byte of b) {
    let c = (crc ^ byte) & 0xff;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    crc = (crc >>> 8) ^ c;
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function makeZip(files: { name: string; data?: string; mode?: number; deflate?: boolean }[]): Buffer {
  const locals: Buffer[] = [];
  const centrals: Buffer[] = [];
  let offset = 0;
  for (const f of files) {
    const name = Buffer.from(f.name);
    const data = Buffer.from(f.data ?? '');
    const method = f.deflate ? 8 : 0;
    const body = f.deflate ? zlib.deflateRawSync(data) : data;
    const l = Buffer.alloc(30);
    l.writeUInt32LE(0x04034b50, 0); l.writeUInt16LE(20, 4); l.writeUInt16LE(method, 8);
    l.writeUInt32LE(crc32(data), 14); l.writeUInt32LE(body.length, 18); l.writeUInt32LE(data.length, 22); l.writeUInt16LE(name.length, 26);
    const c = Buffer.alloc(46);
    c.writeUInt32LE(0x02014b50, 0); c.writeUInt16LE(20, 4); c.writeUInt16LE(20, 6); c.writeUInt16LE(method, 10);
    c.writeUInt32LE(crc32(data), 16); c.writeUInt32LE(body.length, 20); c.writeUInt32LE(data.length, 24); c.writeUInt16LE(name.length, 28);
    c.writeUInt32LE(((f.mode ?? 0o100644) << 16) >>> 0, 38); c.writeUInt32LE(offset, 42);
    locals.push(l, name, body);
    centrals.push(c, name);
    offset += 30 + name.length + body.length;
  }
  const cd = Buffer.concat(centrals);
  const e = Buffer.alloc(22);
  e.writeUInt32LE(0x06054b50, 0); e.writeUInt16LE(files.length, 8); e.writeUInt16LE(files.length, 10);
  e.writeUInt32LE(cd.length, 12); e.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, cd, e]);
}

const limits = { maxEntries: 100, maxBytes: 1 << 20 };
function run(zip: Buffer) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'zipx-'));
  const archive = path.join(dir, 'a.zip');
  fs.writeFileSync(archive, zip);
  const out = path.join(dir, 'out');
  return { dir, out, error: extractZipSafely(archive, out, limits) };
}

describe('zip path safety', () => {
  it('rejects unsafe names', () => {
    for (const n of ['../evil', 'a/../../evil', '/etc/passwd', 'C:\\x', 'a\\b', 'a\0b', '']) {
      expect(unsafeZipEntryName(n)).not.toBeNull();
    }
    expect(unsafeZipEntryName('app/index.js')).toBeNull();
  });
  it('extracts stored and deflated files', () => {
    const r = run(makeZip([{ name: 'app/', mode: 0o040755 }, { name: 'app/a.txt', data: 'hello' }, { name: 'b.txt', data: 'x'.repeat(500), deflate: true }]));
    expect(r.error).toBeNull();
    expect(fs.readFileSync(path.join(r.out, 'app/a.txt'), 'utf8')).toBe('hello');
    expect(fs.readFileSync(path.join(r.out, 'b.txt'), 'utf8')).toHaveLength(500);
    fs.rmSync(r.dir, { recursive: true, force: true });
  });
  it('refuses zip-slip and writes nothing', () => {
    const r = run(makeZip([{ name: 'ok.txt', data: '1' }, { name: '../escape.txt', data: '2' }]));
    expect(r.error).toContain("'..'");
    expect(fs.existsSync(path.join(r.dir, 'escape.txt'))).toBe(false);
    expect(fs.existsSync(path.join(r.out, 'ok.txt'))).toBe(false);
    fs.rmSync(r.dir, { recursive: true, force: true });
  });
  it('refuses symlinks and oversize archives', () => {
    expect(run(makeZip([{ name: 'l', data: '/etc/passwd', mode: 0o120777 }])).error).toContain('symbolic link');
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'zipx-'));
    const f = path.join(dir, 'a.zip');
    fs.writeFileSync(f, makeZip([{ name: 'big', data: 'x'.repeat(100) }]));
    expect(extractZipSafely(f, path.join(dir, 'o'), { maxEntries: 10, maxBytes: 10 })).toContain('size limit');
    fs.rmSync(dir, { recursive: true, force: true });
  });
});
