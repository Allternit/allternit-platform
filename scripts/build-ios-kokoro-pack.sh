#!/usr/bin/env bash
# Build the iPhone Kokoro reply-voice pack (R2 allternit-runtime,
# voice-models/ios/v1/…, served at runtime.allternit.com) and print the upload
# commands. Never uploads. The app pins these exact sha256s and sizes in
# surfaces/allternit-mobile/ios/Core/Voice/KokoroSpeech.swift.
#
#   kokoro/kokoro-v1_0-fp16.safetensors  Kokoro-82M (Apache-2.0): the fp32
#       safetensors kokoro-ios expects (mlx-community/Kokoro-82M-bf16@a71e4d3,
#       fp32 despite the name) stored as fp16. KokoroKit upcasts to fp32 at
#       load, so only the stored weights round (max 0.05% per tensor).
#   kokoro/voices-v1_0-fp16.safetensors  the 28 stock voices (hexgrad,
#       Apache-2.0) from mlalma/KokoroTestApp@9dcd3b0 voices.npz, as fp16.
#   misaki/*  MisakiSwift 1.0.6 Resources (Apache-2.0): US/GB gold+silver
#       lexicons, BART G2P fallback weights + configs, unchanged.
#
# Needs python3 with numpy. Output: $OUT (default ./ios-kokoro-pack).
set -euo pipefail
OUT="${OUT:-$PWD/ios-kokoro-pack}"
WORK="${WORK:-${TMPDIR:-/tmp}/allternit-ios-kokoro}"
PY="${PYTHON:-python3}"
BUCKET="${R2_BUCKET:-allternit-runtime}"
PREFIX="voice-models/ios/v1"
mkdir -p "$OUT/kokoro" "$OUT/misaki" "$WORK"
sha() { shasum -a 256 "$1" | cut -d' ' -f1; }

fetch() { # url sha dest
  if [[ ! -f "$3" || "$(sha "$3")" != "$2" ]]; then
    curl -fL --retry 3 -o "$3.part" "$1" && mv "$3.part" "$3"
  fi
  [[ "$(sha "$3")" == "$2" ]] || { echo "SHA MISMATCH $3" >&2; exit 1; }
}

fetch https://huggingface.co/mlx-community/Kokoro-82M-bf16/resolve/a71e4d38b236d968966a2002c4c895dbd12b1c3c/kokoro-v1_0.safetensors \
  4e9ecdf03b8b6cf906070390237feda473dc13327cb8d56a43deaa374c02acd8 "$WORK/kokoro-v1_0.safetensors"
fetch https://github.com/mlalma/KokoroTestApp/raw/9dcd3b06468a3c1ecee6d09a33ca687c8e708566/Resources/voices.npz \
  56dbfa2f2970af2e395397020393d368c5f441d09b3de4e9b77f6222e790f10f "$WORK/voices.npz"
MISAKI=https://github.com/mlalma/MisakiSwift/raw/6835a1ce4a8854075c89f18ff75c74b13ef58e15/Resources
while read -r name want; do
  fetch "$MISAKI/$name" "$want" "$OUT/misaki/$name"
done <<'LIST'
us_gold.json 8507f89840f0813b10cf584740942f58e9cc9ad3660e24088b442ab0a6b126be
us_silver.json ea0e1abca0c9b18fb0d3402034633a337154a3153e9a9f49f97d668c908e140c
us_bart.safetensors dc4a02e62d4fcb4bb4097ecf00db89b8e1a12a549a52ab6adfbba220b80a55c5
us_bart_config.json 8deb3537fb29c63cd9f20d75515ae06e4c92f1b6db0703a2d45bca95b33a53a4
gb_gold.json 090cdecbe6bda9516a2cdc21563d62def0b91f5b55ea74fe6ef729eef682785f
gb_silver.json cb8655f5e614e6306146e0da1bf93522ac542ebd39dcd9892afcf51c9aa52e1a
gb_bart.safetensors 4994f474bb6f4584076a4e98189caaec11aa3f773a0d82d5bc263a03dd07e703
gb_bart_config.json e4f248e6af0cfb6cb54aea6ff0168d16b6f3cdabed438c487cde4ab0815d1bac
LIST

# f32 safetensors / npz -> f16 safetensors (sorted keys: deterministic bytes).
"$PY" - "$WORK" "$OUT/kokoro" <<'PYEOF'
import io, json, struct, sys, zipfile
import numpy as np
work, out = sys.argv[1], sys.argv[2]
def read_st(path):
    with open(path, 'rb') as f:
        n = struct.unpack('<Q', f.read(8))[0]; header = json.loads(f.read(n)); body = f.read()
    return {k: np.frombuffer(body[v['data_offsets'][0]:v['data_offsets'][1]], dtype=np.float32).reshape(v['shape'])
            for k, v in header.items() if k != '__metadata__'}
def read_npz(path):
    with zipfile.ZipFile(path) as z:
        return {n.removesuffix('.npy'): np.load(io.BytesIO(z.read(n))) for n in z.namelist()}
def write_f16(arrays, path):
    header, blobs, off = {}, [], 0
    for k in sorted(arrays):
        b = np.ascontiguousarray(arrays[k].astype(np.float16)).tobytes()
        header[k] = {'dtype': 'F16', 'shape': list(arrays[k].shape), 'data_offsets': [off, off + len(b)]}
        blobs.append(b); off += len(b)
    h = json.dumps(header, separators=(',', ':')).encode(); h += b' ' * ((8 - len(h) % 8) % 8)
    with open(path, 'wb') as f:
        f.write(struct.pack('<Q', len(h))); f.write(h); [f.write(b) for b in blobs]
write_f16(read_st(f'{work}/kokoro-v1_0.safetensors'), f'{out}/kokoro-v1_0-fp16.safetensors')
write_f16(read_npz(f'{work}/voices.npz'), f'{out}/voices-v1_0-fp16.safetensors')
PYEOF

check() { [[ "$(sha "$OUT/$1")" == "$2" ]] || { echo "OUTPUT SHA MISMATCH $1: $(sha "$OUT/$1")" >&2; exit 1; }; }
check kokoro/kokoro-v1_0-fp16.safetensors 67d4b879ce0f0bc6d30169cca8dc424f192cbed5be811078d8ef7c4a34a34b1f
check kokoro/voices-v1_0-fp16.safetensors e9c828fec6c3f1b2637a0f12ad5e6755d05c28d1091da7c132dac5b15abae332

cd "$OUT"
for f in kokoro/* misaki/*; do
  echo "$f $(stat -f %z "$f" 2>/dev/null || stat -c %s "$f") $(sha "$f")"
done
echo "# upload (not run):"
for f in kokoro/* misaki/*; do
  echo "wrangler r2 object put $BUCKET/$PREFIX/$f --file $OUT/$f --remote --content-type application/octet-stream"
done
