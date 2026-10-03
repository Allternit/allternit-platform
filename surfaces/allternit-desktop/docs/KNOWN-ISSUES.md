# Desktop — Known Issues

Known issues in the Allternit Desktop packaged app (build + run). Last
audited 2026-09-19 against the current source; two of the four issues from the
2026-09-08 packaging run are **fixed in code** and are retained below as
resolved entries for the record.

Log evidence lives in `~/Library/Application Support/@allternit/desktop/main.log`.

---

## 1. Voice service binary crashes at startup — **replaced**

- **Was:** The pyinstaller-bundled `allternit-voice-service` crashed on import (`pyexpat` built for macOS 26.0, host 23.6); Voice Mode unavailable.
- **Now:** Desktop spawns the Rust `voice-service` sidecar, which runs STT (Moonshine/Parakeet via sherpa-onnx) and TTS (Kokoro) in-process. Python/pyinstaller and whisper.cpp are gone. Gizzi Code and the desktop composer expose `/voice` (Ctrl+Space / F8 hold-to-talk). Voice model packs download on first use into `~/.allternit/models/voice/` (not git-vendored).
- **Remaining:** First-run needs network once to fetch the `small` pack (~39 MB: Silero VAD + Moonshine tiny + Smart Turn) for dictation and the `tts` pack (~350 MB, Kokoro v1.0) on first read-aloud; previously: (Silero VAD + Moonshine tiny + Kokoro int8) unless already on disk. The `accurate` pack (Parakeet, ~487 MB) downloads on first accurate-STT request.

## 2. ACU computer-use gateway exits immediately — **FIXED (2026-09-19 audit)**

- **Was:** The gateway's `launch.py` ran under the system `python3`, which lacks `uvicorn` — the process exited right away and the computer-use surface never came up.
- **Fix:** `src/main/acu-gateway-manager.ts` now resolves a python interpreter that actually has `uvicorn` before spawning: `ALLTERNIT_ACU_PYTHON` env override → the staged `resources/computer-use/acu/.venv` → a venv discovered by walking `domains/computer-use/core/.venv` from the resources path / repo root / executable path (`pythonHasUvicorn` probes each candidate). Only a verified interpreter is used.

## 3. Port 8014 `EADDRINUSE` on app start (orphaned connector sidecar) — **FIXED (2026-09-19 audit)**

- **Was:** The bundled connector sidecar bound fixed port 8014; a stale `node` process (old LaunchAgent, shared-checkout dev server) holding that port wedged the app into an EADDRINUSE crash loop at startup.
- **Fix:** `src/main/connector-sidecar-manager.ts` no longer binds a fixed port — the sidecar listens on an **ephemeral loopback port** (`PORT=0`) and announces the real port on stdout; the manager probes `/health` on that port before declaring readiness. Nothing can race port 8014 anymore. Crashes are supervised with exponential backoff and bounded restarts, after which the sidecar is DEGRADED and surfaced in service state instead of silently respawning.

## 4. Mesh fabric enrollment returns 502 — **cloud-side; appears addressed, live behavior unverified**

- **Was:** Fabric enrollment from the desktop failed with a cloud-side `502` during enrollment (the `mesh-node` binary itself runs fine locally). Root cause was server-side, not in the desktop client.
- **Current state:** The cloud enrollment endpoint exists and is implemented in `cmd/allternit-cloud-api/src/routes/mesh.rs` (`POST /api/v1/mesh/enroll`, Clerk-session gated; answers 503 `mesh_not_configured` when `HEADSCALE_API_KEY` is unset). Desktop-side, `src/main/mesh-manager.ts` reports enrollment errors but has no retry/backoff — a failure is logged and mesh starts without a tailnet.
- **Unverified:** Whether the live `api.allternit.com` enrollment path now returns 200 for a signed-in user (needs a Clerk session; cannot be checked from the repo). If enrollment still 502s, it is a cloud-api/infra issue — file it there, not against desktop.
