# Track H: integrate engine + Voice Session + call worker into one branch (headless)

You are a headless Claude executor; nobody can steer you. Orchestrator: the voice session (joe-9e).

Rules:
- Worktree `allternit-ao-voice-integrate`, branch `ao/voice-integrate` (starts at `origin/ao/voice-engine-p1`). Never run git in `~/Desktop/allternit-workspace/allternit`.
- `CARGO_TARGET_DIR=$HOME/Desktop/allternit-workspace/.voice-target-h`; rm -rf it at the end. Don't set RUSTC_WRAPPER/CARGO_BUILD_JOBS (a machine-wide cap is on).
- Push the branch; no PR, no merge, no deploy.

Steps:
1. **Merge the branches:** `git merge --no-ff origin/ao/voice-session`, then `git merge --no-ff origin/ao/voice-callworker`. Resolve conflicts mechanically, following `docs/VOICE_SESSION_NOTES.md` ("Wiring once Phase 1 merges") and `docs/VOICE_CALLWORKER_NOTES.md`:
   - `Cargo.toml` deps: dedupe `sha2`, keep the `call-worker` feature and the optional livekit deps.
   - `lib.rs` mod list: `session`, `call_worker`, plus the engine's.
   - `server.rs`: take the engine's `VoiceServiceState::new`; add the session route.
   - `main.rs`: keep `into_make_service_with_connect_info::<SocketAddr>()`, the `worker`/`serve` subcommands and the engine's `no_espeak` shim. All three must survive.
   - `Cargo.lock`: regenerate.
   - docs: keep every side's sections.
   - Each branch's `docs/*_NOTES.md` / `*_TASK.md` files: keep them. They don't conflict by name.
2. **Wire the Voice Session onto the real engine** (B's notes, steps 1–6; the engine already added hooks in `5cbed3348`, see `docs/VOICE_ENGINE_PHASE_1_NOTES.md` "Track B integration"):
   - Make the sherpa adapter always compiled: drop the `sherpa` feature gate; `ort` + `sha2` become plain deps.
   - Share the server's loaded engines: `router_with(SessionRouteState::new(Arc::new(SherpaEngine::new(...shared Arc<SttEngine>/Arc<TtsEngine>/packs...)), token))`, not a second model load.
   - Use the engine's VAD-less streaming STT entry if it exists; otherwise keep `transcribe_segment`.
   - Use the engine's public VAD builder with turn-taking params (min_silence 0.2 s, min_speech 0.1 s).
   - Use streaming TTS (the chunk callback through allternit-tts).
   - Take the Smart Turn model from the engine's pack manifest (`small` pack). Remove the session's own downloader, or point it at the pack path.
   - Remember TTS is now a child process (`allternit-tts`). The session's `Tts` trait goes through the engine's `TtsEngine`.
3. **Verify everything:**
   - `cargo test -p voice-service` and `cargo test -p voice-service --features call-worker`.
   - `cargo clippy -p voice-service --all-targets -- -D warnings`, and again with `--features call-worker`.
   - `cargo test -p allternit-tts` if it has tests.
   - Build release `-p voice-service -p allternit-tts`, then run `scripts/check-voice-no-gpl.sh` on the voice-service binary. It must pass.
   - **Real end to end:** models are in `~/.allternit/models/voice/` (the small and tts packs; if missing, let the service download them). Start the service locally, then make a WAV question with `say -o q.aiff "What time is it in Tokyo right now?"` and convert it with ffmpeg to 16 kHz mono wav. Run `cargo run --release -p voice-service --example voice_session_client -- --wav q.wav --say "It is three in the afternoon in Tokyo." --out reply.wav`.
     - Record the event timeline: `speech.started` → `transcript.final` text → `turn.ended` (confidence and ms after speech stop) → `speak.started` → first audio ms → `speak.ended`.
     - Check `reply.wav` is real speech (non-silent, duration > 1 s).
     - Also do a barge-in run if the client supports it (B's notes); report `speak.interrupted` timing.
   - `python3 surfaces/docs/scripts/check_links.py` → 0 problems.
4. **Notes:** write `docs/VOICE_INTEGRATE_NOTES.md` with status, the merge commits, the conflicts and how you resolved them, the wiring changes, all test/clippy/guard output, and the e2e timeline. Then run `touch docs/VOICE_INTEGRATE_NOTES.sentinel`. If budget runs low, write the notes first.
