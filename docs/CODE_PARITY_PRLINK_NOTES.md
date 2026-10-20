# PRLINK notes — linked PRs per code session (Contract A)

Branch `factory/cp-prlink`. Code only; nothing was built, typechecked or tested (per the map). Docs checks ran: `check_links.py` 0 problems, `scripts/docs-lint.cjs` passed (Mintlify validation included).

## Built
- `cmd/gizzi-code/src/runtime/session/linked-prs.ts` (new, `LinkedPRs` namespace)
  - `Info` zod schema = Contract A `LinkedPR` (OpenAPI ref `LinkedPR`).
  - Detector `detect`/`observe`: shell tools (`bash`, plus vendor names `Bash`, `shell`, `exec_command`, `run_shell_command`, `local_shell`, …) only when the command matches `gh pr create|new|view|edit|ready|merge|reopen|comment`, `gh api …/pulls`, or `git push`; GitHub MCP PR tools (`create_pull_request`, `update_pull_request`, `merge_pull_request`, `get_pull_request`, or names containing github + pull). Extracts `github.com/<o>/<r>/pull/<n>` and `api.github.com/repos/<o>/<r>/pulls/<n>`; `pull/new/<branch>` never matches (needs digits). Reads/webfetch/grep etc. are never scanned. `gh pr create --draft` seeds state `draft`.
  - Persistence: Storage `["session_prs", sessionID]` → `{ prs }`, dedup by url (case-insensitive), per-session in-memory write lock. A PR seen in a subagent session is also recorded on its root session (walks `SessionTable.parent_id`) so the rail row shows it. Deleted in `Session.remove`.
  - Status: `gh api repos/o/r/pulls/n` (gh found on PATH or /opt/homebrew/bin, /usr/local/bin, /usr/bin; 5-min backoff after an auth failure), else REST with `GITHUB_TOKEN`/`GH_TOKEN` (anonymous for public repos). gizzi has no other stored GitHub token, so env is the "existing token". Mapping: merged when `merged_at`/`merged`, closed, draft, open; mergeable `conflict` when `mergeable === false` or `mergeable_state === 'dirty'`, `clean` when `mergeable === true`, else `unknown`. Cache 60s (10 min for merged/closed), max 3 concurrent fetches, 10s timeout, failures keep last values. Never awaited on a turn.
  - Bus event `session.prs.updated { sessionID, prs }` on list change or status change.
- `src/runtime/session/index.ts`: `Session.updatePart` fires `LinkedPRs.observe(part)` via dynamic import (detached, `.catch`) for completed tool parts; `Session.remove` drops the PR file.
- `src/runtime/server/routes/session.ts`: `GET /prs?ids=` (registered first so it beats `/:sessionID`), `GET /:id/prs`, `POST /:id/prs/refresh`, `POST /:id/prs {url}` (400 on non-PR url), `DELETE /:id/prs?url=` (400 without url). Mounted under both `/session` and `/v1/session`. GETs return cached data and kick a background refresh of stale entries.
- `src/runtime/server/routes/agent-compat.ts`: `/v1/agent-sessions/sync` forwards the event as `{ type: "session_prs", session_id, prs }`; `GET /v1/agent-sessions/:id/prs` read mirror.
- SDK (hand-edited, generation needs a build): `packages/sdk/src/gen/types.gen.ts` (`LinkedPr`, `EventSessionPrsUpdated`, `SessionPrs*Data/Responses/Errors`) and `sdk.gen.ts` (`sessionPrsBulk`, `sessionPrsList`, `sessionPrsRefresh`, `sessionPrsLink`, `sessionPrsUnlink`). The compiled `.js`/`.d.ts` siblings and the big `EventSubscribeResponses` union were not regenerated; regenerate the SDK in the orchestrator build to fold them in.
- Docs: `surfaces/docs/guides/code-linked-prs.mdx` (how PRs link, badges, hover list, Open PR (G), refresh, manual link, routes/event/SDK), `surfaces/docs/guides/code-session-list.mdx` (repo grouping, row marks, ⋯ menu with every item and shortcut G/P/U/R/C/A/D, phone sheet), nav in `docs.json` after cowork-session-panel, cross-link from `surfaces/desktop.mdx` Code mode panes.
- Dependency map: `feature:code-linked-prs` in `docs/dependency-map/features.json`.

## For RAIL / PANEL (consumers)
- Base url is the gizzi runtime: `/v1/session/prs?ids=a,b` → `{ bySession }`; live updates via the SSE event `session.prs.updated` (or `session_prs` on the agent-sessions sync feed).
- `title` is `""` and `mergeable` `unknown` until the first status fetch returns; show `owner/repo #n` as the fallback label.

## Unfinished / caveats
- No UI for manual link or refresh in this branch (RAIL/PANEL own the UI); routes exist and are documented. The docs say statuses refresh automatically and describe the routes for manual link/refresh rather than naming a button.
- The docs for the row menu, badges and phone sheet describe RAIL's task spec; if RAIL ships any item differently, adjust `code-session-list.mdx` at merge.
- Cloud sessions: the PR fetch runs wherever gizzi runs; on a cloud runtime it needs `gh` signed in or `GITHUB_TOKEN` in that runtime's env (ENVRT may inject one via environment secrets). Without either, private-repo PRs keep their placeholder values.
- Not built/typechecked/tested: no unit tests were added (code-only rule). Suggested at merge: a small test for `LinkedPRs.detect`/`extract`/`fromGitHub`.
