# Code mode parity — shared map and contracts (2026-10-20)

Eoj wants Allternit Code mode (local and cloud) to match Claude Code Desktop's code session UI.
Five Claude workers build in parallel in separate worktrees. This file is the SAME in every worktree.
The contracts below are fixed: build to them exactly so the pieces fit when the orchestrator merges.

## Rules for every worker
- Code only. NO builds, typechecks, tests, installs (no pnpm/bun/npm/cargo/tsc/vitest). The orchestrator builds once.
- Commit on your branch with clear messages and `git push -u origin HEAD`. Do NOT open a PR, do NOT merge.
- Stay in your scope. Don't edit files owned by another worker (list below) unless the task says so.
- Production mindset: signed-out / first-run, empty, loading and error states; phone sizes; real data only, no mocks or fake rows.
- Match surrounding code style. No tan surfaces (white + --neutral-fill; dark mode = graphite #1F1F1E).
- Every feature ships on Desktop, the ai.allternit.com phone layout (PhoneShell / phone.css, `isPhoneViewport`) and the m.allternit.com PWA (same source).
- Docs ship with the feature: platform workers update `surfaces/docs/` (Mintlify) + nav, run `python3 surfaces/docs/scripts/check_links.py` (0 problems) and `node scripts/docs-lint.cjs` if present. No bare `<word>` in .mdx prose, quote frontmatter values containing ": ". No competitor product names in docs prose.
- When done write `docs/CODE_PARITY_<YOURTAG>_NOTES.md` (what you built, files, anything unfinished and why), commit + push, THEN create the empty sentinel file `docs/CODE_PARITY_<YOURTAG>.sentinel` (do not commit the sentinel).

## Workers and file ownership
| Tag | Repo / worktree | Owns |
|---|---|---|
| RAIL | allternit-ai / allternit-ai-wt-cp-rail | `src/shell/ShellRail.tsx`, `src/shell/rail/*`, `src/fabric-session/phone/PhoneDrawer.tsx`, new `src/lib/code-session-prs.ts`, `src/lib/code-session-state.ts`, session header ⋯ menu in `src/views/code/CodeSessionHeader.tsx` |
| ENVUI | allternit-ai / allternit-ai-wt-cp-envui | `src/views/code/CodeEnvironmentDialog.tsx`, `src/views/code/CodeWorkspaceBar.tsx`, `src/lib/runtime-target.ts`, new `src/lib/code-environments.ts`, session-create path in `src/lib/agents/mode-session-store.ts` (only the bits passing environmentId/repos) |
| PRLINK | allternit / allternit-wt-cp-prlink | `cmd/gizzi-code` PR detection + `/v1/session/*/prs` routes; docs page for linked PRs |
| ENVAPI | allternit / allternit-wt-cp-envapi | `cmd/allternit-cloud-api` environments table/routes/crypto; docs page for cloud environments |
| ENVRT | allternit / allternit-wt-cp-envrt | `cmd/gizzi-code` applying an environment when a session starts (setup script, env vars, secrets, network policy, repos) |

(The right-side session panel — Environment / Context / Connectors / Outputs / Changes cards — is a later worker, PANEL. RAIL and ENVUI export the hooks it will use.)

## Contract A — linked PRs (gizzi-code serve, PRLINK builds, RAIL consumes)
Type:
```ts
type LinkedPR = {
  url: string            // https://github.com/<owner>/<repo>/pull/<n>
  owner: string; repo: string; number: number
  title: string
  state: 'open' | 'draft' | 'merged' | 'closed'
  mergeable: 'clean' | 'conflict' | 'unknown'   // 'conflict' when GitHub says mergeable_state dirty / mergeable false
  additions: number; deletions: number
  updatedAt: string      // ISO
  detectedAt: string     // ISO, when the session first saw it
}
```
- Detection: whenever a session's tool output (bash `gh pr create`, `git push` remote hints, `gh pr view`, MCP GitHub tools) contains a GitHub PR URL for a PR created/pushed in that session, gizzi records it on the session (persisted with the session, dedup by url).
- Status refresh: via `gh api` if `gh` is authenticated, else GitHub REST with the user's GitHub token gizzi already has (if any), else keep last known values. Cache ≥60s per PR; never block a turn on it.
- Routes (same auth as other `/v1/session` routes):
  - `GET  /v1/session/:id/prs` → `{ prs: LinkedPR[] }`
  - `GET  /v1/session/prs?ids=a,b,c` (ids optional = all sessions) → `{ bySession: Record<string, LinkedPR[]> }`
  - `POST /v1/session/:id/prs/refresh` → `{ prs: LinkedPR[] }` (forces refresh)
  - `POST /v1/session/:id/prs` body `{ url }` → manual link; `DELETE /v1/session/:id/prs?url=` → unlink
- Event: emit bus event `session.prs.updated` `{ sessionID, prs }` on the existing SSE event stream when the list or a status changes.

## Contract B — code environments (allternit-cloud-api, ENVAPI builds, ENVUI + ENVRT consume)
Base `/api/v1/code-environments`, Clerk user auth like other user routes; rows scoped to the user (and org if the codebase does that elsewhere).
```ts
type CodeEnvironment = {
  id: string; name: string
  network_access: 'full' | 'limited' | 'none'
  allowed_hosts: string[]           // used when 'limited'; defaults = package registries + github (document the default list)
  secrets: { name: string; hosts: string[]; created_at: string }[]   // NEVER values
  setup_script: string              // bash, runs before the agent starts
  env_vars: string                  // raw .env text
  repos: { url: string; branch?: string }[]   // repos cloned into the workspace
  is_default: boolean; archived: boolean
  created_at: string; updated_at: string
}
```
- `GET /` → `{ environments: CodeEnvironment[] }` (archived excluded unless `?archived=1`). If the user has none, create and return one named "Default" (network_access full, is_default true).
- `POST /` body = editable fields → `CodeEnvironment`
- `PATCH /:id` partial editable fields → `CodeEnvironment`
- `POST /:id/archive` / `POST /:id/unarchive`
- `POST /:id/secrets` `{ name, value, hosts? }` → `CodeEnvironment` (value encrypted at rest with the server's existing secret key mechanism; name must match `^[A-Z_][A-Z0-9_]*$`; upsert by name)
- `DELETE /:id/secrets/:name` → `CodeEnvironment`
- `GET /:id/resolve` — runtime auth only (the runtime-device token used by runtime_pairing routes, and only for devices owned by the env's user) → `CodeEnvironment` + `secret_values: Record<string,string>`. This is the ONLY place values leave the server. Never log values.

## Contract C — starting a cloud session in an environment (ENVUI sends, ENVRT applies)
- Session create request to gizzi `/v1/session` (and session metadata in the ai store) gains optional `environmentId: string` and `repos: { url: string; branch?: string }[]` (session repos override env repos when given).
- On a runtime started with a cloud API token, gizzi on first turn of a session with `environmentId`: `GET {cloud}/api/v1/code-environments/:id/resolve` → clone repos into the session workspace (one dir per repo) → write env vars → run `setup_script` with bash in the workspace (timeout 10 min, output streamed to the session as a "Setup" step) → then start the agent.
- Session status exposed on `GET /v1/session/:id` as `environment: { id, name, status: 'connecting' | 'setting_up' | 'ready' | 'failed', error?: string }` and as bus event `session.environment.updated`.
- Secrets: injected into the environment of tool processes (bash etc.) only; every secret value is redacted (`[secret:NAME]`) from tool output, transcript storage, and anything sent to the model or UI.
- Network access: `none` = tool processes get no egress (proxy that refuses all); `limited` = HTTP(S)_PROXY to a local allowlist proxy (CONNECT host allowlist = allowed_hosts + secret hosts); `full` = no proxy. Document honestly in docs what this enforces (tool processes, not the host machine).
