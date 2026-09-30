# Backend docs notes

## Files written
- `docs/mcp-apps/ARCHITECTURE.md`, `OPERATIONS.md`, `COMMERCE.md`, `DIRECTORY_REVIEW.md`, `SIWC.md`.

## Files moved out of the repo
Copied to `/Users/joe/Desktop/allternit-workspace/docs/mcp-apps-agent-notes/` (no existing file overwritten, so no `-backend` suffix was needed
except `BACKEND_DOCS_TASK.md`, which had no clash either), then `git rm`'d:
`MCP_APPS_HOST_MAP.md`, `MCP_APPS_HOST_PHASE_1A_{TASK,NOTES}.md`, `P1C_{TASK,NOTES}.md`, `P2_NOTES.md`, `P2_SUBMISSION_PACKAGE.md`,
`P3B_NOTES.md`, `P5_{TASK,NOTES}.md`, `P5_LEGAL_DRAFT_OUTLINES.md`, `SIWC_NOTES.md`. The untracked `docs/BACKEND_DOCS_TASK.md` was copied there and deleted
from the worktree. `JEV_*`, `SWARM_*` and other unrelated docs were not touched. No code changed; no tests or builds were run.

## Claims I could not verify from code
- **Sandbox deploy** (Pages project `allternit-mcp-sandbox`, files `sandbox.html`/`sandbox.js`/`_headers`, live at `mcp-sandbox.gizziio.com`,
  no per-app wildcard yet): from the program handoff and TODO files. None of those files or hosts are in this repo.
- **Clerk settings** (CIMD, Include Audience, JWT, PKCE, `agents:read` scope): from the same handoff; dashboard state is not checkable here.
  The code only shows it requires `aud == MCP_PUBLIC_URL` and scope `agents:read`.
- **Gateway routing** (`/api/mcp/apps`, `/api/mcp/sandbox` resolve via the `/api` prefix): taken from the Phase 1A notes; I did not re-resolve the registry.
- **Commerce and P2 route prefixes**: `commerce_routes.rs` documents `/api/v1/commerce/*`; I confirmed the router is merged in `main.rs` but did not trace the exact mount prefix.
- **Migrations "run when an API process next opens a DB"**: `db.rs` embeds `migrations/` with refinery; I did not run it. Production is stated as manual per the task.
- **Test counts / passing tests** from the old notes were not re-run and are not cited in the docs.
- **Web repo pieces** (checkout sheet, `sandbox-client.ts`, `SiwcCard.tsx`) are described from the old notes only.

## Things found while checking (not fixed, docs-only task)
- **OAuth redirect URI mismatch.** `oauth/start` and the CIMD document use `<ALLTERNIT_PUBLIC_BASE_URL>/mcp/oauth/callback`; the callback's fallback
  in `mcp_routes.rs` uses `<ALLTERNIT_API_PUBLIC_URL>/api/mcp/oauth/callback`. The callback reads the redirect URI recorded on the session first, so
  the directory flow should stay consistent, but two different env vars must both be set correctly. Also, `/mcp` (which holds
  `/oauth/callback`) is nested in `main.rs` next to authenticated routes, and I could not confirm the callback is reachable without a Clerk token
  when the auth server redirects the browser to it. Worth a live check before turning on connector OAuth in production.
- **Stale comment.** `commerce.rs` header says a live key is a startup error; `main.rs` only logs "commerce disabled" and routes return 503. The docs follow `main.rs`.
- `ALLTERNIT_PUBLIC_BASE_URL` and `ALLTERNIT_API_PUBLIC_URL` overlap in purpose; not reconciled.

status: done
