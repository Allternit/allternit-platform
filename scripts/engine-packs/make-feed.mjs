#!/usr/bin/env node
// Sign engine packs into a local feed directory (the layout local-engine reads).
//
//   node make-feed.mjs --feed <dir> --key <ed25519.pem> <pack-dir> [<pack-dir> ...]
//
// For each pack dir (with pack.json from make-manifest.py) writes:
//   <feed>/engines/objects/<sha256>.gz                 every file, gzip, by content
//   <feed>/engines/<pack_id>/manifests/<version>.json  byte copy of pack.json (+ .sig)
//   <feed>/engines/<pack_id>/latest.json (+ .sig)      signed pointer, written last
// Signatures: Ed25519 over the exact bytes, base64 (same as runtime-package.cjs).
//
// This only builds a feed on disk. Uploading it to runtime.allternit.com is a separate,
// human-approved release step and is not done here. For local tests sign with a throwaway
// key and run a debug local-engine with ALLTERNIT_ENGINE_PACK_FEED=file://<feed> and
// ALLTERNIT_ENGINE_PACK_DEV_PUBKEY=<base64 SPKI>; release engines accept only the
// production key and https feed.
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';

const args = process.argv.slice(2);
const opt = (name) => {
  const i = args.indexOf(name);
  if (i < 0) return undefined;
  const v = args[i + 1];
  args.splice(i, 2);
  return v;
};
const feed = opt('--feed');
const keyFile = opt('--key');
if (!feed || !keyFile || args.length === 0) {
  console.error('usage: make-feed.mjs --feed <dir> --key <ed25519.pem> <pack-dir>...');
  process.exit(2);
}
const key = crypto.createPrivateKey(fs.readFileSync(keyFile));
const sign = (buf) => crypto.sign(null, buf, key).toString('base64');
const sha256 = (buf) => crypto.createHash('sha256').update(buf).digest('hex');

for (const dir of args) {
  const raw = fs.readFileSync(path.join(dir, 'pack.json'));
  const m = JSON.parse(raw.toString('utf8'));
  if (!/^[A-Za-z0-9._-]+$/.test(m.version) || !/^[A-Za-z0-9-]+$/.test(m.pack_id)) throw new Error(`bad id/version in ${dir}`);
  const objects = path.join(feed, 'engines', 'objects');
  fs.mkdirSync(objects, { recursive: true });
  let bytes = 0;
  for (const [rel, f] of Object.entries(m.files)) {
    const data = fs.readFileSync(path.join(dir, rel));
    if (data.length !== f.size || sha256(data) !== f.sha256) throw new Error(`${dir}/${rel} does not match pack.json`);
    const obj = path.join(objects, `${f.sha256}.gz`);
    if (!fs.existsSync(obj)) {
      const gz = zlib.gzipSync(data, { level: 9 });
      fs.writeFileSync(obj, gz);
      bytes += gz.length;
    }
  }
  const packDir = path.join(feed, 'engines', m.pack_id);
  const manifestRel = `manifests/${m.version}.json`;
  fs.mkdirSync(path.join(packDir, 'manifests'), { recursive: true });
  fs.writeFileSync(path.join(packDir, manifestRel), raw);
  fs.writeFileSync(path.join(packDir, `${manifestRel}.sig`), sign(raw));
  // Also sign pack.json in the pack dir so the installed copy carries its signature.
  fs.writeFileSync(path.join(dir, 'pack.json.sig'), sign(raw));
  const latest = Buffer.from(JSON.stringify({
    format: 2, pack_id: m.pack_id, version: m.version, build: m.build,
    manifest: manifestRel, manifestSha256: sha256(raw),
  }, null, 2));
  fs.writeFileSync(path.join(packDir, 'latest.json.sig'), sign(latest));
  fs.writeFileSync(path.join(packDir, 'latest.json'), latest);
  console.log(`${m.pack_id} ${m.version}: ${Object.keys(m.files).length} files, ${(bytes / 1e6).toFixed(1)} MB new objects`);
}
