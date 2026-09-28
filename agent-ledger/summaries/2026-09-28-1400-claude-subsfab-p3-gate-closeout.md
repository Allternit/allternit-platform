# Attestation — Claude Code (Opus 5.5) — subscription-fabric P3 gate: continue, SSE, image policy, D6 — 2026-09-28 07:20 → 14:00 CDT

Resumed from `docs/specs/subscription-fabric/HANDOFF.md` §9 (session d8af1d82's
handoff). Every result below was run live on gate guest
`computer-e0cc21e903e843b7b2b65de9da471404` (account `2bf94d24…`, Eoj's
ChatGPT Plus) through the scratch allternit-api on :18013.

## Merged

| PR | Merge | What |
|---|---|---|
| #863 | c605b17bc | chat.continue thread mapping (§S6) + ChatGPT `ignoreSend` completion; 4 live-found fixes: acknowledged-then-crash was retryable (resend risk) → `stalled`; orphans settled only on next lane traffic → boot sweep; chat.create typed into the previous task's page (stateless prompt landed IN a mapped thread) → fresh chat; continue read the thread before render → wait; Chrome crash-restore bubble suppressed |
| #873 | 1c2295bde | allternit-api computer proxy streamed nothing (buffered body + 30s total timeout) → streams, SSE exempt; SDK extractor flattened nested paragraphs (ChatGPT document block) → walks containers |
| #881 | 281ee9bd1 | Image-chat policy (Eoj 2026-09-28): images in the "Allternit" ChatGPT project, one reused chat per account, rotate at N (default 20); 6 live UI facts mirrored in fixtures (late sidebar, button entries, hover-gated create, late "+"/menu, late earlier images, null adapter logger) |
| #882 | 9a430999c | D6: `GET /v1/artifacts/:id/download` (non-rendering attachment, sha256 header); `chatgpt-image` skill `fabric_capture.mjs` lane (outside this repo) |

## P3 manual gate — verdicts (IMPLEMENTATION_PLAN §P3)

| Step | Verdict | Evidence |
|---|---|---|
| connect → ready (Firefox login mode) | PASS | #820/#834 (prior session); re-confirmed ready all day |
| chat.create | PASS | temp chat, not in Recents |
| chat.continue on a fabric thread | PASS | "Mango" → "Orange" same provider thread; 409 unmapped; divergence fail on a polluted thread |
| SSE stream | PASS | via API proxy after #873: created → submitted → 57 progress → done → completed in real time; 50s stream kept heartbeats |
| kill -9 mid-stream → restart, no double submit | PASS (gateway + Chrome kill) | task `stalled`, not retryable, 1 attempt, settled at boot. **Eoj's eyeball check in ChatGPT still outstanding** (beekeeper/cartographer/clockmaker chats: one prompt each) |
| image → artifact + sha256 | PASS | PNG 1254²/1536×1024, sha256 == stored name, mode 444; downloadable via #882 |
| image → local preview | NOT DONE | sandboxed preview route not built (bytes download only) |
| D6 media-router lane | PASS | Mac → proxy → guest → Allternit project → PNG, checksum verified |

## Not done / needs Eoj

- Two EMPTY duplicate "Allternit" projects on Eoj's ChatGPT account (created
  by the pre-fix lookup in #881's development). Delete only with his OK.
- Keep or stop the gate guest; kill scratch API :18013 + `rm -rf
  /tmp/sessions-gate` at closeout — NOT done: the D6 lane's only endpoint
  today is that scratch API, so teardown is his call with the guest.
- Durable gateway endpoint + token for day-to-day D6 use (today env-only).
- Open decisions unchanged: guest IPv4 NAT; writable viewer + API
  `origin_gate` port allowlist.
- Old ChatGPT lanes (Safari, Chrome profile) stay until the gateway lane
  runs stable (D6) — nothing retired.

Tests at close: gateway 297/297, SDK 76/76, allternit-api computer_ws 21/21.
