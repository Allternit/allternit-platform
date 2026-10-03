# Track E.1: wire the relay secret + move the verifier to a shared `relay_auth.rs` (headless)

Same rules as `docs/VOICE_RUNTIME_TASK.md`:
- this worktree and branch `ao/voice-runtime`;
- no git in the shared checkout;
- `CARGO_TARGET_DIR=$HOME/Desktop/allternit-workspace/.voice-target-e` (rm -rf at the end);
- don't set RUSTC_WRAPPER/CARGO_BUILD_JOBS;
- commit and push, no PR.
Read `docs/VOICE_RUNTIME_NOTES.md` first. It found that allternit-api never holds its own device token.

Agreed with joe-07 (who owns pairing):
1. **Move the relay verifier** (`sign_relay`, `verify_relay`, `VoiceRelaySecret`, `RelayedVoiceAuth`, header constants, `MAX_SKEW_SECS`) out of `voice_calls.rs` into a new shared module `cmd/allternit-api/src/relay_auth.rs`.
   - Rename it generically: `RelaySecret`, `RelayedAuth`, keeping the type aliases if that helps.
   - joe-07 will reuse it to sign other relayed envelopes (e.g. discord-app).
   - `voice_calls.rs` uses it from there. Tests move with it.
2. **Implement `EnvOrFileRelaySecret`** (production default):
   - First, env `ALLTERNIT_RUNTIME_DEVICE_TOKEN` + `ALLTERNIT_RUNTIME_OWNER_ID` (both must be set).
   - Otherwise, read the identity JSON at `$ALLTERNIT_RUNTIME_IDENTITY_PATH`, default `~/.config/allternit/runtime-identity.json`. Keys: `deviceToken`, `userId`, `runtimeId`, `expiresAt`. allternit-node and agent-daemon read the same file (see `allternit-node/src/config.rs` `identity_path`, `identity.rs`).
   - Re-read on every verify, or cache keyed by file mtime: the token rotates and allternit-node rewrites the file. Treat an expired `expiresAt` as absent.
   - Neither source → None, so the routes keep answering 503 "relay not configured". Never accept unsigned.
   - **Desktop note:** gizzi's `ALLTERNIT_API_TOKEN` is a Clerk session token, NOT the device token. Don't use it.
   - Tests: env wins over the file; file read; mtime change picks up a new token; expired → None; missing/corrupt file → None.
3. **Provisioned cloud computers:** in `infrastructure/provisioned-instance/init.sh` step 3, where `$ENV_FILE` gets `ALLTERNIT_NODE_DEVICE_ID` and `ALLTERNIT_RUNTIME_DEVICE_TOKEN`, add `ALLTERNIT_RUNTIME_OWNER_ID=<owner>`.
   - Source it from the pairing exchange response if it carries the user id; check how init.sh gets the token and what the response contains. If the response lacks the owner, say so precisely in the notes instead of guessing.
   - Make sure the allternit-api service unit/launcher in that image loads `$ENV_FILE` (find it). If it doesn't, add the EnvironmentFile/source line.
   - Touch only those lines.
4. **Verify:**
   - `cargo test -p allternit-api --lib relay_auth voice_calls`;
   - then rebuild and re-run the smoke boot: with a temp identity file → a signed request with the right owner gets past auth (expect 404 unknown-call or 200 on create with a fake resolver if the routes allow it); a wrong owner → 401; no file → 503.
   - Update `cmd/allternit-api/docs/VOICE_CALLS.md` + the Mintlify page.
5. **Notes:** append "E.1 results" to `docs/VOICE_RUNTIME_NOTES.md` (files, tests, smoke output, the init.sh finding), then run `touch docs/VOICE_RUNTIME_1_1.sentinel`.
