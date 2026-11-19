/**
 * Pure-JS zip extraction (node:zlib only) for platforms whose tar cannot read
 * zip (GNU tar on Linux). Mirrors the tar path's protection: entry names are
 * checked before anything is written, symlinks and special entries are refused,
 * sizes are capped, and the resolved path must stay under the destination.
 */
import * as fs from 'node:fs';
import * as path from 'node:path';
import * as zlib from 'node:zlib';

export interface ZipEntry {
  name: string;
  method: number;
  compressedSize: number;
  size: number;
  offset: number;
  isDirectory: boolean;
  unixMode: number;
}

/** Returns an error message when a zip entry name could escape or is malformed. */
export function unsafeZipEntryName(name: string): string | null {
  if (!name) return 'Archive entry has an empty name';
  if (name.includes('\0')) return 'Archive entry contains a null byte';
  if (name.startsWith('/') || /^[A-Za-z]:/.test(name) || name.startsWith('\\\\')) {
    return `Archive entry escapes the install directory: ${name}`;
  }
  if (name.includes('\\')) return `Archive entry contains a backslash: ${name}`;
  if (name.split('/').includes('..')) return `Archive entry contains '..': ${name}`;
  return null;
}

export function readZipEntries(buf: Buffer, maxEntries: number): { entries: ZipEntry[]; error?: string } {
  let eocd = -1;
  for (let i = buf.length - 22; i >= Math.max(0, buf.length - 22 - 65535); i--) {
    if (buf.readUInt32LE(i) === 0x06054b50) { eocd = i; break; }
  }
  if (eocd < 0) return { entries: [], error: 'Not a valid zip archive' };
  const total = buf.readUInt16LE(eocd + 10);
  let pos = buf.readUInt32LE(eocd + 16);
  if (total === 0xffff || pos === 0xffffffff) return { entries: [], error: 'Zip64 archives are not supported' };
  if (total === 0) return { entries: [], error: 'Archive is empty' };
  if (total > maxEntries) return { entries: [], error: `Archive has too many entries (${total})` };
  const entries: ZipEntry[] = [];
  for (let n = 0; n < total; n++) {
    if (pos + 46 > buf.length || buf.readUInt32LE(pos) !== 0x02014b50) {
      return { entries, error: 'Corrupt zip central directory' };
    }
    const flags = buf.readUInt16LE(pos + 8);
    const method = buf.readUInt16LE(pos + 10);
    const compressedSize = buf.readUInt32LE(pos + 20);
    const size = buf.readUInt32LE(pos + 24);
    const nameLen = buf.readUInt16LE(pos + 28);
    const extraLen = buf.readUInt16LE(pos + 30);
    const commentLen = buf.readUInt16LE(pos + 32);
    const externalAttrs = buf.readUInt32LE(pos + 38);
    const offset = buf.readUInt32LE(pos + 42);
    const name = buf.toString(flags & 0x800 ? 'utf8' : 'latin1', pos + 46, pos + 46 + nameLen);
    if (flags & 1) return { entries, error: `Encrypted zip entry: ${name}` };
    entries.push({
      name, method, compressedSize, size, offset,
      isDirectory: name.endsWith('/'),
      unixMode: externalAttrs >>> 16,
    });
    pos += 46 + nameLen + extraLen + commentLen;
  }
  return { entries };
}

/** Extract a zip into destDir. Returns an error string, or null on success. */
export function extractZipSafely(
  archivePath: string,
  destDir: string,
  limits: { maxEntries: number; maxBytes: number },
): string | null {
  const buf = fs.readFileSync(archivePath);
  const { entries, error } = readZipEntries(buf, limits.maxEntries);
  if (error) return error;
  let totalBytes = 0;
  for (const e of entries) {
    const bad = unsafeZipEntryName(e.name);
    if (bad) return bad;
    const type = e.unixMode & 0o170000;
    if (type === 0o120000) return `Archive created a symbolic link: ${e.name}`;
    if (type && type !== 0o100000 && type !== 0o040000) return `Archive created an unsupported entry type: ${e.name}`;
    totalBytes += e.size;
    if (totalBytes > limits.maxBytes) return 'Archive expands beyond the size limit';
  }
  const root = path.resolve(destDir);
  fs.mkdirSync(root, { recursive: true, mode: 0o700 });
  for (const e of entries) {
    const target = path.resolve(root, e.name);
    if (target !== root && !target.startsWith(root + path.sep)) {
      return `Archive entry escapes the install directory: ${e.name}`;
    }
    if (e.isDirectory) {
      try { fs.mkdirSync(target, { recursive: true }); } catch (err) { return `Could not create ${e.name}: ${err instanceof Error ? err.message : String(err)}`; }
      continue;
    }
    if (e.offset + 30 > buf.length || buf.readUInt32LE(e.offset) !== 0x04034b50) return 'Corrupt zip local header';
    const start = e.offset + 30 + buf.readUInt16LE(e.offset + 26) + buf.readUInt16LE(e.offset + 28);
    const raw = buf.subarray(start, start + e.compressedSize);
    let data: Buffer;
    try {
      if (e.method === 0) data = raw;
      else if (e.method === 8) data = zlib.inflateRawSync(raw, { maxOutputLength: e.size + 1 });
      else return `Unsupported zip compression method ${e.method}: ${e.name}`;
    } catch (err) {
      return `Could not extract ${e.name}: ${err instanceof Error ? err.message : String(err)}`;
    }
    if (data.length !== e.size) return `Zip entry size mismatch: ${e.name}`;
    const exec = (e.unixMode & 0o111) !== 0;
    try {
      fs.mkdirSync(path.dirname(target), { recursive: true });
      // wx: a duplicate entry name fails instead of overwriting an earlier file.
      fs.writeFileSync(target, data, { mode: exec ? 0o755 : 0o644, flag: 'wx' });
    } catch (err) {
      return `Could not write ${e.name}: ${err instanceof Error ? err.message : String(err)}`;
    }
  }
  return null;
}
