# Attestation — session/kimi-subs-keychain (Kimi Code), 2026-09-26 ~16:20 CDT

**PR #769 (merge 2866add9b) — subscription-gateway pluggable secret-store backend (D3/D15).**

## What was done

The one blocker between the merged P3 activation and the cloud gate: the
gateway's D3 boot gate hard-required the local macOS Keychain
(`requireKeychain` in main.ts), so it could not boot on a Linux/Windows
Sessions machine. The spec anticipated this (`keychain.ts:4` D15 note) and the
seam already existed (`KeychainBackend` interface, injectable
`boot({ keychain })`). This PR:

- `SUBS_GATEWAY_KEYCHAIN=file|keychain` (default `keychain`; unknown value
  fails config load loudly). Off-macOS the default still fails closed —
  running with the file store is an explicit opt-in, so D3 stays structural.
- `FileKeychainBackend` (`src/security/keychain.ts`): 0600 `keychain.json`
  under the state dir, atomic tmp+rename writes, corrupt store = outage
  (never a silent clobber), restart-safe.
- `selectKeychainBackend({ kind, stateDir })`; main.ts wires config →
  selection; `deps.keychain` injection still wins (all existing boot tests
  unchanged).
- Docs alignment: README "Keychain requirement (D3)" → "Secret store
  (D3/D15)"; the stale "It must never run on cloud infrastructure" line
  replaced with the D15 placement rules (T1 Hosted / T2 BYOC / T3
  Local-contained; single-tenant always; never uncontained on a
  daily-driver desktop). HARDENING.md D3 row marked refined with the D15
  pointer. Comment sweep in main.ts / tokens.ts.

## Honest caveat (stated in code, README, and PR body)

File-backend values are **plaintext at rest** (0600 filesystem permissions
only). This deviates from REVIEW_CLAUDE.md:484 ("keep secrets in the macOS
Keychain, not in a file") deliberately and only on the contained
single-tenant Sessions machine per D3-refined (D15). Encrypt-at-rest via the
master key (§A6.3) is a follow-up, not shipped.

## Verification evidence

- `pnpm -F subscription-gateway build` — PASS
- `CI=1 pnpm -F subscription-gateway test` — **253/253** (28 files; +9 new in
  `test/keychain.test.ts`: file-backend roundtrip / 0600 mode / cross-instance
  persistence / corrupt-store outage / master-key flow, backend selection,
  config parse + loud reject). Both real-browser suites green (warm Chrome).
- `scripts/check-fabric-sources-clean.sh` (NUL gate) — OK
- Provider-literal grep on added lines — clean. No new files. CLI untouched.
- Desktop rebuild step N/A: subscription-gateway is not bundled into the
  desktop app (grep of surfaces/allternit-desktop scripts/resources empty).

## Session notes / incidents

- Session opened on the HANDOFF pivot: independently verified the previous
  session's repair claim — `git diff e79d9a639..e036f0dbc` = HANDOFF.md only
  (144+/142-, one file). Repair holds.
- Owner decisions this session: gate path = Option A (T1 Hosted cloud
  computer). Owner clarified the taxonomy: A is the production way when a
  user has a cloud desktop provisioned; T3 local-contained is the future
  own-computer tier where the VM is controlled in the background (nothing
  visible on the daily-driver desktop). Owner asked what the ~30¢/hr cloud
  charge is: it is our own rate card (`pricing.rs` `computer_minute` 0.5
  ¢/min linux) metered against our own org's credits ledger on our own Incus
  VPS — no external provider bill. Cloud-computer creation was NOT performed
  this session (keychain PR came first; create command + cost will be shown
  before any creation, per the spend gate).
- Deviation (precedent: session/backend-fixes): CommRails WIH DAG skipped —
  CLI not on PATH, build cost.
- Deviation: attestation landed via docs PR (not direct main commit) — the
  shared checkout was mid-attestation by a concurrent live session
  (session/backend-fixes, uncommitted LEDGER.md); direct commit there would
  have touched another session's dirty files.
- Shared-checkout state at session end: main == origin/main, dirty files all
  belong to concurrent sessions (backend-fixes et al.) — untouched by me.

## Next (unchanged from HANDOFF §3)

Cloud gate runbook: spend go-ahead from Eoj (he now knows it's internal
metering, not an external bill) → `allternit computers create --kind
cloud_desktop --os linux ... --name sessions` → install Node/pnpm/Chrome deps
→ boot with `SUBS_GATEWAY_KEYCHAIN=file SUBS_GATEWAY_TCP=1 tsx src/main.ts` →
Eoj drives the ChatGPT login via the streamed display → gate steps → verdict
attestation → stop the computer.
