# Gateway — add Gemini and Copilot as carried vendors (platform side)

## Hard rules (read first)
- Work ONLY in this worktree (`allternit-wt-gemini-copilot`, branch `gateway/gemini-copilot-adapters`). Never run git commands in `~/Desktop/allternit-workspace/allternit` (the shared checkout) or any other worktree. Ignore any steering / git-discipline hook warnings about other checkouts. Never delete branches. Never use `git stash`. Stay strictly on this task.
- Do not edit `.steering/`, CI workflows, `agent-ledger/`, or migrations of other features.
- Do NOT open a browser to the vendors, sign in anywhere, or send any message to a real vendor. No live vendor traffic from this session. Everything here is built and proven offline against fixtures.
- Never auto-approve a vendor prompt, never solve a bot check (see `docs/gateway/ADAPTERS.md` rules). A bot-check page returns `LANE_BLOCKED`; selector drift returns `ADAPTER_DRIFT`.
- Do not copy vendor logos or proprietary assets from the web. Use a monogram (`logoAsset: null`) unless an asset already exists in the repo.
- Rust builds: use the shared target dir (`CARGO_TARGET_DIR=$HOME/Desktop/allternit-workspace/.shared-target` if it exists, else `.gw-target`); do not create a fresh `target/` in this worktree. Check `df -h /System/Volumes/Data` first; stop and report if under 40 GB free.
- Fix siblings with the same flaw in this change and list them in the notes.
- Commit to this branch. Do NOT push, open a PR, or merge.

## Background
The Agent Gateway carries vendors through adapters in `services/subscription-gateway/adapters/<id>/`. Read first: `docs/gateway/ADAPTERS.md`, `docs/gateway/ARCHITECTURE.md`, `docs/gateway/LOOK_PACKS.md`, `docs/gateway/OPERATIONS.md` (live verification checklist), `docs/specs/subscription-fabric/CAPABILITY_INVENTORY.md`.

The newest vendor, **Kimi**, is the template. It has two parts:
- `adapters/kimi-web/` — the subscription-fabric web chat adapter: `manifest.yaml`, `selectors/v1.yaml`, `adapter.ts`, `fixtures/{idle,streaming,complete,logged-out,challenge,limit-banner}.html`. Tested by `services/subscription-gateway/test/web-chat-adapters.test.ts`.
- `adapters/kimi-subscription/` — the AAI adapter over that login: `index.ts`, `aai.ts`, `fixtures/offline.ts`.
Find every other place Kimi is wired (`git grep -n -E "kimi-subscription|kimi-web|\"kimi\"|'kimi'"` across `services/`, `platform/packages/`, `cmd/allternit-api/src/` — notably `cmd/allternit-api/src/agent_gateway_routes.rs`) and mirror each one.

## Build
Two vendors, each with the same two parts and the same wiring as Kimi:

| Vendor | web adapter | AAI adapter | login URL | vendor id |
|---|---|---|---|---|
| Gemini (Google) | `adapters/gemini-web/` | `adapters/gemini-subscription/` | `https://gemini.google.com/app` | `google` |
| Copilot (Microsoft) | `adapters/copilot-web/` | `adapters/copilot-subscription/` | `https://copilot.microsoft.com/` | `microsoft` |

For each:
1. `manifest.yaml`, `adapter.ts`, `selectors/v1.yaml` with an ordered fallback list per named key (composer input, send, stop, assistant message, streaming indicator, logged-out marker, challenge/bot-check marker, usage-limit banner, account identity), `critical` flags as Kimi's. In the selectors file, mark EVERY selector `inferred` (not verified live) with a comment saying what it was inferred from; prefer stable hooks (roles, aria-labels, data attributes, element names) over class names.
2. Fixtures: the same six pages as Kimi, hand-written minimal HTML that matches the selectors, plus `look-profile.json` if Kimi's adapters ship one.
3. AAI adapter (`index.ts`, `aai.ts`, `fixtures/offline.ts`): capability manifest truthful for a plain chat lane (`ui_bridge`, `best_effort`, one context at a time unless Kimi's says otherwise), pacing like Kimi's, `termsWarning` text for a UI-bridge lane.
4. Wiring: every registry, allowlist, plan table, usage-reading hook, identity reader, sign-in domain list and Rust route table where Kimi appears. Sign-in must cover Google's and Microsoft's login domains (`accounts.google.com`; `login.live.com`, `login.microsoftonline.com`) the way the existing sign-in flow handles multi-domain logins.
5. Tests: extend `web-chat-adapters.test.ts` and the adapter guard/conformance tests so both vendors pass offline conformance (`POST /aai/conformance/<id>?offline=1` path) for every area their manifest supports.
6. Docs: add both to the adapter status table in `docs/gateway/ADAPTERS.md` as "offline conformance passing; selectors inferred; NOT verified live", and add `adapters/<id>/README.md` for each with the exact live-verification steps a human must run (sign in under Settings → Subscriptions on a Sessions computer, then the live checklist).

## Verify
- `pnpm -C services/subscription-gateway typecheck:noemit` clean and `pnpm -C services/subscription-gateway test` passes (install first the way the repo documents it).
- If any Rust file changed: `cargo check -p allternit-api` with the shared target dir, and boot the real binary once on a fresh database to prove routes do not overlap (the repo has a smoke-boot recipe; search docs for "smoke"). Report the result.
- No file under `adapters/` of another vendor changed, except shared registries.

## Deliverable
`docs/GEMINI_COPILOT_NOTES.md`: per-file changes; every place Kimi was wired and what you mirrored; the two adapter ids and vendor ids; test/typecheck/cargo summary lines; the list of selectors that need live confirmation; anything not done and why. End with `status: done`. Then create empty `docs/GEMINI_COPILOT_NOTES.sentinel`.
