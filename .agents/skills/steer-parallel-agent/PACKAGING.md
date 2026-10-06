# Packaging agent orchestration + steering into the Allternit Platform / gizzi-code

> How the orchestration and steering tooling (the Allternit Factory, the `steer-*`
> scripts, the agent-orchestrator skill, the steer-parallel-agent skill) ships in
> the product.

The pre-Factory orchestrator scripts and work-engine binaries are gone (removed,
not aliased). Their replacements are in the Factory
[migration page](../../../surfaces/docs/factory/migration.mdx).

## What exists and where it lives

| Asset | Location | Scope |
|---|---|---|
| Orchestration (spawn, send, watch, capture, stop agents) | The Allternit Factory: `gizzi agents up\|ps\|down\|recover\|doctor`, `gizzi orchestration send\|capture\|transcript`. Engine binary `allternit-factory`, shipped next to `gizzi` and bundled by Desktop | Product |
| `agent-orchestrator` skill (delegate to external CLI agents) | `.agents/skills/agent-orchestrator/` (project scope) and the gizzi-code bundled catalog | Product |
| `steer-parallel-agent` skill (redirect an already-running session) | `.agents/skills/steer-parallel-agent/` (project scope) and the gizzi-code bundled catalog | Product |
| `steer-*` scripts (`steer`, `steer-discover`, `steer-context`, `steer-checkpoint`, `steer-prompt`, `steer-verify`) | `tools/agent-orchestrator/scripts/`, symlinked to `~/.local/bin/` by `tools/agent-orchestrator/install.sh` | Desktop |
| Steering and peer endpoints | `/api/factory/steer/{checkpoint,consult,commit-gate}` and `/api/factory/peers…`, served in process by `cmd/allternit-api` | Product |
| Peer registration in gizzi-code | `cmd/gizzi-code/src/runtime/gizzi-core/services/railsPeer.ts`, default on | Product |

## Skills: three channels

gizzi-code skill loading supports all three:

| Channel | Path | When to use |
|---|---|---|
| Bundled | `cmd/gizzi-code/src/runtime/skills/bundled/` via `bundledSkills.ts` | Compiled into the CLI; ships to every install |
| Project | `<repo>/.agents/skills/` | Loads for any agent that opens the repo |
| Workspace | `<workspace>/.allternit/skills/**` | Loaded by `src/workspace/loader.ts`. For user-level workspaces |

For kimi-code, `config.toml` supports `extra_skill_dirs = []`; the installer can
append the platform repo's `.agents/skills/` path so kimi sessions pick up the
same skills.

## Steering endpoints

`steer-checkpoint` posts to `/api/factory/steer/checkpoint`; the repo's
`.steering/` Stop hook and commit gate consult through
`gizzi orchestration steer consult` (see `.steering/README.md`). Nothing else to
package.

## Session discovery

`steer-discover` asks the Factory peer registry first
(`GET <allternit-api>/api/factory/peers`; peers carry `name`, `cwd`, `vendor`,
`last_heartbeat_at`, `status`), then falls back to scanning local session files
(`~/.kimi-code/sessions/*/state.json`) for agents that don't register. Terminal
bots started with `gizzi agents up` are registered automatically.

## Steering hooks: per-repo convention

The `.steering/checkpoint.md` convention plus the Stop-hook steering consult is
documented in the platform `AGENTS.md`. Repos opt in by adding the hook config and
a `.steering/README.md`.

## Open items

- Homebrew **formula** for the CLI tools: needs release tarballs with sha256; the
  desktop cask (`homebrew-tap/Casks/allternit.rb`) bundles the tools into the
  app's resources instead.
- `install.allternit.com`: the DMG installer should run the toolkit install step
  post-install (owned by the websites repo; coordinate before editing).

## What NOT to package

- Session-specific state (`~/.kimi-code/sessions/`, wire logs): machine-local by design.
- The Factory API itself: already part of `cmd/allternit-api`, runs as a platform service.
- Evidence and log dirs under `~/.allternit/factory/`: runtime output, not product code.
