# Third-Party Notices: allternit-tts

`allternit-tts` is licensed GPL-3.0-or-later (see `LICENSE`) because it
statically links espeak-ng.

## Linked into the binary (via the `sherpa-onnx-sys` 1.13.8 static libraries)

| Component | Licence |
|---|---|
| sherpa-onnx (+ the `sherpa-onnx` / `sherpa-onnx-sys` crates) | Apache-2.0 |
| onnxruntime | MIT |
| **espeak-ng** and its `ucd` library | **GPL-3.0-or-later** |
| piper-phonemize | MIT |
| kaldi-native-fbank, kaldi-decoder, OpenFst, sentencepiece | Apache-2.0 |
| kissfft | BSD-3-Clause |

Source for the native libraries: https://github.com/k2-fsa/sherpa-onnx (tag
v1.13.8), which builds espeak-ng (https://github.com/espeak-ng/espeak-ng) and
piper-phonemize (https://github.com/rhasspy/piper-phonemize) from source.

Rust crates: serde and serde_json (MIT/Apache-2.0).

## Model (downloaded at runtime, not part of the binary)

| Model | Licence |
|---|---|
| Kokoro-82M v1.0 (sherpa-onnx export `kokoro-multi-lang-v1_0`, fp32), incl. its `espeak-ng-data` | Apache-2.0 (model); espeak-ng-data GPL-3.0-or-later |
