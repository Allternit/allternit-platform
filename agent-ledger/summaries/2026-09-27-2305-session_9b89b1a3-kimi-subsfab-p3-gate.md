# Attestation — session_9b89b1a3 (kimi) — subscription-fabric P3 gate unblock, 2026-09-26 21:00 → 09-27 23:05 CDT

## What was done

Resumed the P3 cloud gate from `docs/specs/subscription-fabric/HANDOFF.md` §6.
The staged single blocker turned out to be five. All fixed, tested, merged:

1. **PR #785 (f1d7100f1)** — `SubstrateRouter` did not forward
   `guest_service_url` (trait default 501'd the proxy + PTY lanes). Added
   forwarding + 2 tests (scripted-HTTP IncusDriver forward; no-driver
   routing-error). `cargo test -p allternit-computer-cloud router` 7/7.
2. **PR #786 (1c4f7ed86)** — guest setup died at the gateway D3 boot gate
   ("local keychain is not available"): attestation commit 850a31905 had
   committed a stale session tree, reverting PR #769's secret-store backend
   (88 files total). Restored the 7 files `aab43b553` touched, byte-exact
   (no commits touched them between aab43b553 and 850a31905^). typecheck +
   keychain tests 13/13. Parallel session's PR #787 healed the other 81.
3. **PR #789 (1046ba101)** — live-smoke found `guest_service_url` returning
   `http://legacy:<port>` (synthetic single-host pool name used as vnc_host).
   Added `IncusDriver::reachable_host` (real-URL hosts keep derived vnc_host;
   synthetic names fall back to the driver-level INCUS_VNC_HOST); applied to
   `guest_service_url` (both paths) and `get_desktop_endpoint` (same latent
   bug). Updated the 3 tests that self-fulfillingly derived expected URLs
   from the wrong field. `cargo test -p allternit-computer-cloud` 104/104.
4. **PR #791 (a203ece63)** — next live-smoke failure: gateway A6.1 host guard
   403'd `forbidden_host` (hop-by-hop stripping drops Host; reqwest derived
   mail.news...:<port>). `proxy_forward` now sets `Host: 127.0.0.1:<guest_port>`.
   `cargo test -p allternit-api computer_ws` 19/19.
5. Gate infrastructure: rebuilt the setup bundle correctly
   (`git archive --format=tar.gz` — plain redirect writes an uncompressed
   tar), discovered the Incus files API 404s on missing parent dirs
   (misleading "Execution not found" — diagnosed via a logging proxy that
   proved the request was correct), created a new gate computer
   `computer-e0cc21e903e843b7b2b65de9da471404` THROUGH a scratch
   allternit-api on :18013 (worktree build, scratch DB, own tokens, Incus
   env from ~/.allternit/incus-host.env, ALLTERNIT_DESKTOP_WS_SECRET set),
   uploaded the bundle, ran setup: Node 22 + pnpm 10 + gateway healthy on
   UDS with the file keychain; cli-token minted; chatgpt-web adapter live.
   Proxy lane proven end-to-end from the Mac (proxied `/v1/health` → ok).
   `subs connect chatgpt` done: account `2bf94d24-02a4-4947-8afb-f56cfe01a85a`,
   session_health `auth_required`.

## Verification evidence

- Router/vnc-host/proxy-host: crate test suites above (7/7, 104/104, 19/19)
  plus the live proxied-health round trip through :18013.
- Keychain restore: `pnpm run typecheck` PASS; `vitest run test/keychain.test.ts` 13/13.
- Full HANDOFF state (infrastructure map, gate steps remaining, trap list):
  `docs/specs/subscription-fabric/HANDOFF.md` §8.

## Incidents

- **850a31905 stale-attestation corruption** (88 files reverted by a
  docs(ledger) commit from another session): healed by #786 (gateway files)
  and parallel #787 (the rest). Flagged as a live pattern in HANDOFF §8.4.9.
- Shared checkout `~/Desktop/allternit-workspace/allternit` cannot
  fast-forward: uncommitted stale AGENTS.md edit (drops the "one current
  build" commandment) from another session blocks `git pull --ff-only`.
  Left untouched per worktree-ownership rules; needs owner inspection.

## Honest deferrals (gate NOT finished)

- ChatGPT login (Eoj-driven, writable noVNC through :18013) was in progress
  when the session paused; task run chat.create, kill -9 adoption check, and
  image.generate + quarantine remain. Exact next steps: HANDOFF §8.3.
- Old guest `computer-9bd5cfe494a140d182645e645039f74b` still exists
  (redundant) — delete is a gated action, queued for the next session.
- Closeout cleanup queued per HANDOFF §8.3.6 (scratch API, /tmp/sessions-gate,
  /tmp/subs-scratch-api, merged session branches/worktrees).
- Observability debt from §3 (`let _ = wait_operation` in substrate.rs create)
  still open.
