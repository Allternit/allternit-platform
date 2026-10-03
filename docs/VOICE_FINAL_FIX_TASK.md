# Fix: final transcript worse than the last interim (headless)

Same rules as `docs/VOICE_INTEGRATE_TASK.md`:
- this worktree and branch `ao/voice-integrate`;
- no git in the shared checkout;
- `CARGO_TARGET_DIR=$HOME/Desktop/allternit-workspace/.voice-target-h` (rm -rf at the end);
- don't set RUSTC_WRAPPER/CARGO_BUILD_JOBS;
- push, no PR.

**Bug** (see `docs/VOICE_INTEGRATE_NOTES.md` "Findings"): in the Voice Session, the final decode of a segment includes the ~0.2 s of trailing silence the turn-taking VAD needs, and Moonshine tiny garbles it. The last interim said "What time is it in Tokyo right now?", but the final said "A time is at in Tokyo right now." `POST /v1/stt` on the same audio is correct.

**Fix in `services/voice/src/session/`** (the sherpa adapter / core): before the final decode, trim trailing (and leading) low-energy audio beyond a short pad. Keep ~0.1 s, and use the VAD's speech end or a simple RMS threshold. Make the final decode match the HTTP route's segment handling if that differs (compare what `/v1/stt` does in `stt.rs`).
- Also add a guard: if the final has a markedly lower word overlap with the last interim, and the interim was produced from ≥ 90% of the segment, prefer re-decoding the trimmed audio. Don't silently swap texts without a re-decode.
- Keep it simple, with unit tests for the trim.

**Verify on real audio, not just one sample:**
- Generate 8 different questions with macOS `say` using 3+ voices (`say -v Samantha|Daniel|Karen …`), including one with background noise mixed in via ffmpeg. Convert to 16 kHz mono wav.
- Run each through `voice_session_client` against a locally started service (models are in `~/.allternit/models/voice/`; use `PORT=18001` since 8001 may be taken), before and after the fix.
- Report the final transcript vs the source text for each, plus the WER before and after.
- Also run the bench's STT part to confirm no regression: `cargo run --release -p voice-service --example voice_bench -- --data ~/.allternit/voice-bench --only stt`.

Tests and clippy (both feature sets) pass. Append a "Final-transcript fix" section to `docs/VOICE_INTEGRATE_NOTES.md`, then run `touch docs/VOICE_FINAL_FIX.sentinel`.
