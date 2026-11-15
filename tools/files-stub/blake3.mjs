// Minimal BLAKE3 (default hash mode, 32-byte output) in plain JS. Node 20, no dependencies.
// Follows the reference implementation: 1 KiB chunks, 64-byte blocks, binary tree of parent nodes.
const IV = new Uint32Array([0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19]);
const PERM = [2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8];
const CHUNK_START = 1, CHUNK_END = 2, PARENT = 4, ROOT = 8;
const rotr = (x, n) => ((x >>> n) | (x << (32 - n))) >>> 0;

function g(s, a, b, c, d, mx, my) {
  s[a] = (s[a] + s[b] + mx) >>> 0; s[d] = rotr(s[d] ^ s[a], 16);
  s[c] = (s[c] + s[d]) >>> 0; s[b] = rotr(s[b] ^ s[c], 12);
  s[a] = (s[a] + s[b] + my) >>> 0; s[d] = rotr(s[d] ^ s[a], 8);
  s[c] = (s[c] + s[d]) >>> 0; s[b] = rotr(s[b] ^ s[c], 7);
}

const st = new Uint32Array(16);
const m = new Uint32Array(16);
const tmp = new Uint32Array(16);
// Returns the 16-word state after 7 rounds (before the final xor folding).
function compress(cv, block, counter, blockLen, flags) {
  st.set(cv.subarray(0, 8), 0);
  st.set(IV.subarray(0, 4), 8);
  st[12] = counter >>> 0; st[13] = Math.floor(counter / 4294967296) >>> 0; st[14] = blockLen; st[15] = flags;
  m.set(block);
  for (let r = 0; r < 7; r++) {
    g(st, 0, 4, 8, 12, m[0], m[1]); g(st, 1, 5, 9, 13, m[2], m[3]);
    g(st, 2, 6, 10, 14, m[4], m[5]); g(st, 3, 7, 11, 15, m[6], m[7]);
    g(st, 0, 5, 10, 15, m[8], m[9]); g(st, 1, 6, 11, 12, m[10], m[11]);
    g(st, 2, 7, 8, 13, m[12], m[13]); g(st, 3, 4, 9, 14, m[14], m[15]);
    if (r < 6) { for (let i = 0; i < 16; i++) tmp[i] = m[PERM[i]]; m.set(tmp); }
  }
  for (let i = 0; i < 8; i++) { st[i] ^= st[i + 8]; st[i + 8] ^= cv[i]; }
  return st;
}

function words(buf, off, len) {
  const w = new Uint32Array(16);
  const padded = Buffer.alloc(64);
  buf.copy(padded, 0, off, off + len);
  for (let i = 0; i < 16; i++) w[i] = padded.readUInt32LE(i * 4);
  return w;
}

// Output of a chunk's last block (or a parent) that has not been folded yet.
function chunkOutput(buf, start, len, counter) {
  let cv = IV.slice(0, 8);
  const nBlocks = Math.max(1, Math.ceil(len / 64));
  for (let b = 0; b < nBlocks - 1; b++) {
    const flags = b === 0 ? CHUNK_START : 0;
    cv = Uint32Array.from(compress(cv, words(buf, start + b * 64, 64), counter, 64, flags).subarray(0, 8));
  }
  const last = nBlocks - 1;
  const bl = len - last * 64;
  return { cv, block: words(buf, start + last * 64, bl), counter, blockLen: bl, flags: (last === 0 ? CHUNK_START : 0) | CHUNK_END };
}
const outCv = (o) => Uint32Array.from(compress(o.cv, o.block, o.counter, o.blockLen, o.flags).subarray(0, 8));
const parentOutput = (l, r) => { const w = new Uint32Array(16); w.set(l, 0); w.set(r, 8); return { cv: IV.slice(0, 8), block: w, counter: 0, blockLen: 64, flags: PARENT }; };

/** blake3-256 of a Buffer, lower-case hex. */
export function blake3hex(buf) {
  const stack = [];
  const nChunks = Math.max(1, Math.ceil(buf.length / 1024));
  let output;
  for (let i = 0; i < nChunks; i++) {
    const start = i * 1024;
    const o = chunkOutput(buf, start, Math.min(1024, buf.length - start), i);
    if (i === nChunks - 1) { output = o; break; }
    let cv = outCv(o);
    let total = i + 1;
    while ((total & 1) === 0) { cv = outCv(parentOutput(stack.pop(), cv)); total >>= 1; }
    stack.push(cv);
  }
  while (stack.length) output = parentOutput(stack.pop(), outCv(output));
  const r = compress(output.cv, output.block, output.counter, output.blockLen, output.flags | ROOT);
  const out = Buffer.alloc(32);
  for (let i = 0; i < 8; i++) out.writeUInt32LE(r[i], i * 4);
  return out.toString('hex');
}

if (process.argv[1] && import.meta.url.endsWith(process.argv[1].split('/').pop()) && process.argv.includes('--test')) {
  const v = [['', 'af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262'], ['abc', '6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85']];
  let bad = 0;
  for (const [s, h] of v) if (blake3hex(Buffer.from(s)) !== h) { bad++; console.error('blake3 FAIL', JSON.stringify(s)); }
  console.log(bad ? 'blake3: failed' : 'blake3: vectors ok');
  process.exit(bad ? 1 : 0);
}
