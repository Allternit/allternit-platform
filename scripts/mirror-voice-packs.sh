#!/usr/bin/env bash
# Mirror the pinned voice-pack files to R2 (allternit-runtime/voice-packs/v1/<file>).
#
# Downloads each pinned upstream file, verifies its sha256 (the same pins as
# services/voice/src/models.rs), then uploads it. Without credentials it only
# downloads, verifies, and prints the upload commands. With R2_ENDPOINT,
# R2_ACCESS_KEY and R2_SECRET set it performs them (curl --aws-sigv4).
#
#   R2_ENDPOINT=https://<account>.r2.cloudflarestorage.com \
#   R2_ACCESS_KEY=... R2_SECRET=... scripts/mirror-voice-packs.sh
#
# Keep the MANIFEST below in sync with PACKS in models.rs (name|sha256|url).
set -euo pipefail

BUCKET="${R2_BUCKET:-allternit-runtime}"
PREFIX="voice-packs/v1"
CACHE="${VOICE_MIRROR_CACHE:-${TMPDIR:-/tmp}/allternit-voice-mirror}"
mkdir -p "$CACHE"

read -r -d '' MANIFEST <<'LIST' || true
silero_vad.onnx|9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6|https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx
sherpa-onnx-moonshine-tiny-en-quantized-2026-02-27.tar.bz2|9ec31b342d8fa3240c3b81b8f82e1cf7e3ac467c93ca5a999b741d5887164f8d|https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-moonshine-tiny-en-quantized-2026-02-27.tar.bz2
smart-turn-v3.2-cpu.onnx|2bb026316b14a660486a75b1733cd3fbab8c2fd0314dc9af7be49f8cca967e4f|https://huggingface.co/pipecat-ai/smart-turn-v3/resolve/f766f81d3cfdf7737ac64aad813d91bbfd56bf93/smart-turn-v3.2-cpu.onnx
kokoro-multi-lang-v1_0.tar.bz2|c5f7e2d2caf082bc1d20fb70334a61d99d20b484500aad32e7cf84c128ea3298|https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kokoro-multi-lang-v1_0.tar.bz2
bundle.json|bab643150f437f37df080a710520ff39ed9ebd9a339f8ebdc739f7eddfc28b3f|https://huggingface.co/KevinAHM/pocket-tts-onnx/resolve/58a6d00cf13d239b6748cb0769f35c580a8f606c/onnx/english_2026-04/bundle.json
tokenizer.model|d461765ae179566678c93091c5fa6f2984c31bbe990bf1aa62d92c64d91bc3f6|https://huggingface.co/KevinAHM/pocket-tts-onnx/resolve/58a6d00cf13d239b6748cb0769f35c580a8f606c/onnx/english_2026-04/tokenizer.model
bos_before_voice.npy|f46edf4f7007b7ba4ea58831f49d003e59e167b4641c44bb3addfe9231a780b1|https://huggingface.co/KevinAHM/pocket-tts-onnx/resolve/58a6d00cf13d239b6748cb0769f35c580a8f606c/onnx/english_2026-04/bos_before_voice.npy
mimi_encoder.onnx|853e2ca623b8782d94c3745ec6133bfdff7ce33d9b11128bd29ea03f28d76e3d|https://huggingface.co/KevinAHM/pocket-tts-onnx/resolve/58a6d00cf13d239b6748cb0769f35c580a8f606c/onnx/english_2026-04/mimi_encoder.onnx
text_conditioner.onnx|4ecee995fb69f85c7a7493d11f7b5ee15d9950facc7ab3f5c9c49ef1e03847bb|https://huggingface.co/KevinAHM/pocket-tts-onnx/resolve/58a6d00cf13d239b6748cb0769f35c580a8f606c/onnx/english_2026-04/text_conditioner.onnx
flow_lm_main_int8.onnx|f9bd8106b79a0192c1c43399ab938fb24900a95c1c599870d75a884e99000116|https://huggingface.co/KevinAHM/pocket-tts-onnx/resolve/58a6d00cf13d239b6748cb0769f35c580a8f606c/onnx/english_2026-04/flow_lm_main_int8.onnx
flow_lm_flow_int8.onnx|3dd781ee5abee9e195320bf0106bebd6372a852b3b36352524ee78b40554635d|https://huggingface.co/KevinAHM/pocket-tts-onnx/resolve/58a6d00cf13d239b6748cb0769f35c580a8f606c/onnx/english_2026-04/flow_lm_flow_int8.onnx
mimi_decoder_int8.onnx|3630450a3297a101792a6ac66619ebc70ab916b265e6220c2afaef8b1673f925|https://huggingface.co/KevinAHM/pocket-tts-onnx/resolve/58a6d00cf13d239b6748cb0769f35c580a8f606c/onnx/english_2026-04/mimi_decoder_int8.onnx
sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2|5793d0fd397c5778d2cf2126994d58e9d56b1be7c04d13c7a15bb1b4eafb16bf|https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2

LIST

apply=0
if [[ -n "${R2_ENDPOINT:-}" && -n "${R2_ACCESS_KEY:-}" && -n "${R2_SECRET:-}" ]]; then apply=1; fi

sha_of() { shasum -a 256 "$1" | cut -d' ' -f1; }

while IFS='|' read -r name sha url; do
  [[ -z "$name" ]] && continue
  f="$CACHE/$name"
  if [[ ! -f "$f" || "$(sha_of "$f")" != "$sha" ]]; then
    echo "download $name"
    curl -fL --retry 3 -o "$f.part" "$url"
    mv "$f.part" "$f"
  fi
  got="$(sha_of "$f")"
  if [[ "$got" != "$sha" ]]; then
    echo "SHA MISMATCH $name: expected $sha got $got" >&2
    rm -f "$f"
    exit 1
  fi
  echo "verified $name"
  cmd=(curl -fS --aws-sigv4 "aws:amz:auto:s3" --user "\$R2_ACCESS_KEY:\$R2_SECRET"
       -H "Content-Type: application/octet-stream" -T "$f"
       "\${R2_ENDPOINT}/$BUCKET/$PREFIX/$name")
  if [[ $apply -eq 1 ]]; then
    curl -fS --aws-sigv4 "aws:amz:auto:s3" --user "$R2_ACCESS_KEY:$R2_SECRET" \
      -H "Content-Type: application/octet-stream" -T "$f" \
      "$R2_ENDPOINT/$BUCKET/$PREFIX/$name"
    echo "uploaded $PREFIX/$name"
  else
    echo "would run: ${cmd[*]}"
  fi
done <<<"$MANIFEST"

if [[ $apply -eq 0 ]]; then
  echo "dry run: set R2_ENDPOINT, R2_ACCESS_KEY, R2_SECRET to upload."
fi
