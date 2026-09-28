# chatgpt-web

ChatGPT web UI adapter for the Subscription Gateway (`ui_bridge_web`). Covers
`chat.create`, `chat.continue`, `image.generate` (pools `chat-msgs` /
`image-gen`). Built on the SDK `DeclarativeChatAdapter` with code hooks for
image capture (`captureImages` → artifact store), `chat.continue` divergence
checks against `thread_mappings` fingerprints (`on_divergence: adopt|fork|fail`,
default `fail`), and fingerprint-based `reconcile` (never resubmits).

**Fresh chat:** `chat.create` and `image.generate` first navigate to the
origin root (a new regular chat), never reusing the page the previous task
left — otherwise a prompt lands in that task's thread or temp chat.
`freshChat: false` is for fixture tests that load a page directly.

**Temp chat (D5):** a stateless `chat.create` (no `thread_id`) clicks the
temporary-chat toggle by default (`new ChatGPTWebAdapter({ tempChat: false })`
or future per-account policy to opt out); a threaded one stays in a regular,
reopenable chat. Plans without the toggle run in normal history.

**Selectors are v1-unverified.** `selectors/v1.yaml` was written from public
knowledge of the ChatGPT web UI and has NOT been validated against a logged-in
session; the conformance fixtures are shaped to these selectors, so the suite
proves internal consistency, not live fidelity.

**Manual gate (Phase 3, human-driven):**
1. `allternit subs connect chatgpt` — the worker opens the Sessions window; log
   in by hand; probe should report READY.
2. `allternit task run chat.create --prompt "Say hello" --wait` — reply streams
   over SSE; task completes.
3. `kill -9` the gateway mid-stream, restart — reconcile adopts the thread or
   flags `submission_ambiguous`; verify in the provider UI that nothing was
   double-submitted.
4. `allternit task run image.generate --prompt "…" --wait` — artifact row with
   sha256 lands; `allternit artifacts open <id> --i-know-its-quarantined`.

Challenges/login walls surface as `needs_user` and are never auto-retried
(Critical #5). A profile held by another process maps to `profile_locked`
(fix #6).
