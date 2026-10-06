# docs/ — taxonomy and rules

How this directory is organized (ratified S0, enforced S6/S7 — 2026-09-18).

## The taxonomy

1. **Stable dir families do not move.** Capitalized families (`Core_System/`, `Operations/`, `Future_Blueprints/`, `Business_Strategy/`, `Product_and_Content/`, `Archive/`) and stable lower-case dirs (`architecture/`, `audit/`, `design/`, `marketing/`, `pipeline/`, `projects/`, `public/`, `research/`, `specs/`, `reports/`, `upstream/`, `learning/`, `archive/`, `parity-reports*/`, `openai-audit/`, `kimi-audit/`, `a-protocol/`, `agent-activity-design/`, `agent-tasks/`, `Audits_and_Research/`, `desktop-cloud-mvp/`, `gap-analysis/`, plus small dirs `assets/ bots/ demos/ development/ infra/ jobs/ plans/`) are the filing cabinet. Reorganizing one is an explicit decision, not a cleanup.
2. **`programs/<program>/`** holds phase-organized program docs (Swarm builds, the pre-Factory work engine, gizzi-code, iOS, cloud-agents, AO/UHP, ACU shadow head, media-plugins). Filename prefixes map to programs (`SWARM_*`, `RAILS_*`, `GIZZI_*`, `IOS_*`, `CLOUD_AGENTS_*`, `ACU_*`, `AO_*`, `MEDIAPLUG*`).
3. **`learnings/`** holds one-off docs with no clear program prefix — triage notes, audit tasks, setup guides, runbooks.
4. **Depth 1 stays almost empty.** Only `MASTER_INDEX.md` (this index's sibling), plus docs that the root `AGENTS.md` references by frozen path (`NATIVE_SESSIONS.md`, `AGENT_EMAIL_RAIL.md`).

## Rules

- **New docs land in the right shelf on arrival**, not at depth 1. If you don't know the shelf, use `learnings/` and say why in the doc.
- **Move with `git mv`** and fix inbound links in the same PR — a moved doc with stale inbound links is worse than no move.
- **[agent-ledger/](../agent-ledger/LEDGER.md) is the signed session record and deliberately lives outside docs/; docs link to it, never copy from it.** Its historical summaries are dated records: never edit them to paper over link rot — count the stale references and note them instead.
- **Root has its own keepers.** `GIZZI.md` (workspace-instruction file the runtimes load from cwd), `THIRD-PARTY-NOTICES.md` (electron-builder extraFiles in the desktop release), and `spec/Contracts/` (read from disk by validate_law.py, the context-pack-builder driver, and the gateway service) stay at repo root because live code reads them there. Root `.md` files otherwise live in `docs/`.
- **`MASTER_INDEX.md` is refreshed whenever the structure changes.** Keep its format; bump the refresh date.
