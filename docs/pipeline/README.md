# .pipeline — Discovery-to-spec pipeline

An automated loop that watches AI/agent-systems sources, writes mechanism
briefs for the most relevant new items, announces them over rails mail, and
(later phases) turns them into verified specs for executor agents.

## Phase roadmap

- **Phase 1: scout** — discover → brief → announce.
  - `bin/rails-ensure.sh` — proves rails (port 8013) accepts pipeline writes,
    self-healing via the dev API; hard-fails otherwise (no fallback).
  - `bin/scout.cjs` — fetches sources via `.github/scripts/lib/pipeline.cjs`,
    writes briefs for the top-5 unseen items to `briefs/`, tracks slugs in
    `seen.json`, announces each to rails thread `wih:pipeline-discovery`.
- **Phase 2: generator** — deterministic brief → spec (OpenSpec +
  EARS + Gherkin, no LLM).
  - `bin/generate-spec.cjs` — parses the structured brief format and emits
    `docs/pipeline/specs/<slug>.md`; strict (malformed briefs rejected, never
    improvised), deterministic (byte-identical regen), manifest at
    `specs/.generated.json`.
- **Phase 3: spec-checker + queue** — READY/NEEDS-WORK review loop
  (3-round cap), `docs/pipeline/queue/` for executor consumption, memory
  ingestion of rejection patterns.
  - `spec-rubric.md` — the reviewer's prompt: verdict-able requirements,
    acceptance that proves its requirement, right-sizing, phase boundaries.
  - `bin/check-spec.sh` — consults the independent reviewer (`allternit-factory orchestration steer consult`;
    `SPEC_CHECK_CMD` overrides for tests) per unverdicted spec. READY →
    `queue/` + rails announce to `wih:pipeline-queue` (hard error on
    failure); NEEDS-WORK → findings in `specs/<slug>.review.md`, 3 rounds →
    STALLED; 2nd NEEDS-WORK ingests the rejection pattern to memory (:3201,
    advisory only). Verdicts in `verdicts.json`.
  - **B3: rails tickets are the queue's native store.** READY → after the
    announce + `mv`, `check-spec.sh` creates a rails ticket (`POST
    /api/rails/tickets`: title = first heading, kind `feature`, labels
    `["pipeline","spec:<slug>"]`, queue path + brief provenance in
    `description` — the only free-text field) and merges `ticket_id` into
    `verdicts.json` (merge semantics: a later `verdict_set` never wipes it).
    Ticket failure = hard error, but gates ticket creation only — the spec
    stays in `queue/` and builds legacy. Frontmatter `blocks: [<slug>, …]`
    (passed through from briefs by `generate-spec.cjs`) wires dependency
    edges blocker → ticket via `POST /tickets/:id/dependencies`; a 409 cycle
    rejection is logged + flagged in `errors.log` (non-fatal).
- **Phase 4 (this): queue consumption** — spawn build executors for READY
  specs; human merge is the boundary.
  - `bin/build-queue.sh` — for each queued spec not already `building`/
    `built` in `builds.json`: generate `builds/<slug>-TASK.md` (executor
    conventions + full spec content), record `building`, `ao-spawn
    --worktree build-<slug>` (`BUILD_AGENT_CMD`, default `kimi --yolo`),
    `ao-send` the task-file path inside the worktree, then `ao-watch` the
    NOTES sentinel. DONE → record `built` + announce to
    `wih:pipeline-builds` (`built: <slug> — awaiting human merge review`);
    watch exit 3/4 → record `failed` + announce `failed: <slug>`.
    Announcements are tracked separately (`announced: false` until a rails
    2xx): an announcement failure is a hard error, and a later run retries
    only the announce — never re-spawns a completed build.
    `--no-wait` spawns without watching.
    **B3:** `--all` builds ticketed items first — `GET
    /api/rails/tickets/ready` filtered client-side for the `pipeline` label,
    ordered by `GET /api/rails/graph/triage` score (tickets missing from the
    50-capped triage response sort after scored items by `created_at` then
    ticket_id), then legacy ticket-less queue files; tickets not in the ready
    list (blocked) are skipped. If the tickets endpoint is unreachable it
    logs and degrades to legacy file mode. On the watch verdict: built →
    `POST /tickets/:id/close` (reason = NOTES path); failed → ticket stays
    open + failure note appended via PATCH; then `record-outcome.sh <slug>
    merged|failed` (C4 wiring). Both are advisory (logged, non-fatal).
    There is NO auto-merge — a human merges `ao/build-<slug>` after review.

- **Taste memory loop (C1+C4)** — the pipeline learns the project's taste.
  - `bin/taste-ingest.sh` — builds the taste corpus: ingests repo docs
    (`AGENTS.md`/`DESIGN.md`/`README.md` + top-level `docs/*.md`) and the
    allternit-brain wiki as `trusted`, plus agent session transcripts
    (`TASTE_SESSIONS`) tiered by `taste/trust-rules.json` (default
    `unverified`; paths matching `revert`/`failed` → `failed`). Every item is
    POSTed to memory (:3201, advisory) with
    `{source, trust_tier, provenance_ref}`; `taste/ingested.json` ledgers
    content hashes so re-runs skip unchanged items. Failed approaches stay
    visible as pitfalls, never as evidence.
  - `bin/record-outcome.sh <slug> <merged|reverted|rejected|failed> [note]` — the
    outcome feedback loop. **When a human merges, reverts, or rejects a build
    at the queue/merge stage, record the decision with this command** (the
    build-queue announce step stays as-is; this is the human's half of the
    loop; B3 also wires build-queue to call it with `merged`/`failed` when the
    watch verdict lands). Appends `{ts, slug, outcome, note}` to
    `outcomes.jsonl` and ingests the outcome to memory as a taste precedent
    (`merged` → `trusted`, `reverted`/`rejected`/`failed` → `failed`).
  - Precedent staleness: `check-spec.sh`'s `query_precedents` marks memory
    items older than 90 days `[stale]` in the assembled precedent text instead
    of presenting them as current (undated items degrade to current), and
    labels `failed`-tier items `[pitfall]` so reverted/rejected attempts stay
    visible but never read as evidence (C1-R2's consult half).

- **Wiki connector + artifact contracts (C2+C3)** — the brain wiki becomes a
  candidate source, and every pipeline artifact carries a schema-versioned
  contract.
  - `bin/wiki-ingest.sh` — reads the brain wiki (`TASTE_BRAIN`, default
    `$HOME/Desktop/allternit-brain`, skipped silently when absent). Pages with
    frontmatter `type: idea|pain` become `candidates/<slug>.md` (frontmatter:
    `source_page`, `trust_tier: unverified`, `ingested_at`); everything else
    (runbook/decision/identity/domain/no frontmatter) is context only. ALL
    pages are ingested to memory (advisory; idea/pain `unverified`, context
    `trusted`; ledger keys `wiki:<relpath>`). ENFORCEMENT-ONLY (C2-R1): the
    wiki is read-only, page content is never executed, and injection text in a
    page changes nothing but candidate creation.
  - `bin/dismiss.sh <slug-or-title> [note]` — records a dismissal in
    `dismissals.json` (`{slug: {title, dismissed_at, note}}`) and ingests it
    to memory as a `failed`-tier taste precedent (C2-R3). The scout
    suppresses items whose normalized title (lowercase, alnum-only) matches a
    dismissal younger than 14 days — logged to `errors.log` with the
    dismissal cited; after 14 days the item may surface again.
  - Artifact contracts (C3-R1): briefs (scout), specs (generate-spec), and
    verdict review records (check-spec) all carry frontmatter with
    `schema_version: 1`, `trust_tier`, `provenance_refs`, `produced_by`,
    `produced_at`. Briefs cite the source URL; specs cite the brief path +
    brief SHA-256; reviews cite the spec path. `bin/contract-test.sh` pins
    the contract with golden fixtures in `taste/golden/` and validates the
    live producers against it.
  - Note: spec regeneration is byte-identical modulo the wall-clock
    `produced_at` frontmatter line (masked in determinism comparisons).

## The charter (taste layer)

`docs/pipeline/charter.md` decides **what kind of features the pipeline may build** —
what Allternit is, what we build, what we do NOT build, current priorities.
It is plain data you edit; the pipeline applies it at machine speed:

- The scout frames every brief through the charter.
- The spec-checker includes the charter (plus taste precedents queried from
  memory) in every review and has a third verdict: `REJECT` — charter
  violation, final, spec moves to `docs/pipeline/rejected/` and the violation is
  ingested to memory immediately. (`NEEDS-WORK` is for fixable specs;
  `REJECT` is for features we should never build.)
- Rejections become taste precedents: future consults see past rejection
  decisions, so pipeline taste converges on yours.

## The learning loop (M1)

The pipeline learns **skills**, not just facts. Two deterministic triggers,
mirroring how humans learn (program: `docs/pipeline/PROGRAM-meta-learning.md`):

- **Learnable moments** are captured at the moment they happen: every
  steering/gate/check-spec verdict, every `record-outcome.sh` call, every
  dismissal appends `{ts, kind, refs, summary}` to
  `docs/pipeline/learn/events.jsonl` (gitignored) via the shared helper
  `bin/learn-event.sh <kind> <refs> <summary>`. Capture is additive and
  advisory — it never alters a verdict or gate decision.
- **Reflection points** fire when a run completes (end of `check-spec.sh`
  and `build-queue.sh`): `bin/learn-reflect.sh` reads the events since the
  last reflection (watermark in `docs/pipeline/learn/watermark`), consults
  `allternit-factory orchestration steer consult` with the distillation prompt
  (`docs/pipeline/learn/reflect-prompt.md`; `LEARN_CONSULT_CMD` overrides the
  consult for tests), and appends the returned rules to
  `docs/pipeline/playbook.md` — each rule imperative, with `confidence`,
  `provenance` (event refs), `added`, and `last_confirmed`. Reflection is
  advisory: a consult failure is logged and the watermark stays put, so no
  events are lost.
- **Consumption**: every steering consult (`steer_build_context`) and every
  check-spec request includes the playbook via `bin/learn-playbook.sh`,
  capped at 4KB; rules unconfirmed for 90+ days are marked `[stale]` at
  inclusion time (the playbook file itself is never mutated by inclusion).
- **Persistence (M3)**: each distilled rule is also written to the resolved
  brain (`bin/brain-resolve.sh`, shared with taste/wiki-ingest: TASTE_BRAIN →
  gizzi settings `brain.path` → ~/brain → skip) as `learnings/<slug>.md`
  with frontmatter `type: lesson`, `status: active|stale`, `domain`,
  `confidence`, `provenance_refs`, `added`, `last_confirmed`. Rules 90+ days
  unconfirmed flip to `status: stale` on the next run — aging is visible in
  the brain. When the brain is a git repo, each reflection run makes ONE
  `learn:` commit (`add -- learnings` only, never pushes — sync is the
  user's `gizzi brain sync`). With no resolvable brain the skip is noted
  once in errors.log and the playbook still updates. The brain is written
  ONLY under `learnings/`.
- **Audit before adoption (M2)**: when a rule's provenance shows 3+ events
  of the same kind, reflection marks it `upgrade_candidate: true` in the
  playbook and writes `docs/pipeline/proposals/<slug>.md` (frontmatter
  `target_artifact`, `kind`, `evidence_event_ids` + a fenced proposed
  change). `bin/audit-proposal.sh` is the ONLY path by which the pipeline
  modifies itself: it consults the auditor against `proposal-rubric.md`
  (evidence support, minimal scope, charter conflict) and verdicts
  ADOPT/REVISE/REJECT into `proposals/verdicts.json` (merge semantics;
  REVISE re-audits only after the proposal actually changes). ADOPT on a
  data target (`.steering/prompt.md`, `docs/pipeline/playbook.md`,
  `docs/pipeline/*-rubric.md` — computed from the path, never trusted) appends
  the change under an adoption marker and commits `learn: adopt proposal
  <slug>`; ADOPT on a code target emits `proposals/tasks/<slug>-TASK.md`
  for a real executor instead. Every adoption links slug → commit in
  `outcomes.jsonl`; REJECT feeds the taste memory.

## Learning metrics (M5)

"Better" is measured, not felt. `bin/metrics.sh` computes the learning
system's quality signals from the EXISTING logs — read-only, no behavior
changes to any script:

- **Sources**: `.steering/state/consults.log` (gate + checkpoint consult
  lines), `docs/pipeline/learn/events.jsonl`, `docs/pipeline/outcomes.jsonl`, and
  git history (commits per ISO week).
- **Metrics** (per ISO week where applicable): (a) first-pass rate — % of
  gated commits approved on first attempt; (b) gate block rate (STEER
  share); (c) reviewer verdict distribution
  (APPROVE/STEER/REJECT/CONSULT_FAILED); (d) executor stall signals — nudge
  events (none emitted yet; wired for when they appear) + gate STEER →
  same-cmd APPROVE gaps > 10 min; (e) outcome linkage coverage — % of
  adopted proposals with a recorded outcome.
- **Honesty**: a metric with < 3 relevant events reports
  `insufficient_data` — never a fake number. No smoothing.
- **Outputs**: `docs/pipeline/metrics/latest.json` (machine) + `latest.md`
  (human, one paragraph per metric with a trend arrow), both gitignored;
  the summary is ingested to memory as an insight (advisory — memory down =
  log + continue). `docs/pipeline/metrics/history.jsonl` is appended ONLY when
  the values change, so re-runs are idempotent.

```bash
bash docs/pipeline/bin/metrics.sh        # compute + write latest.* + ingest
bash docs/pipeline/bin/metrics-test.sh   # fixture tests, hand-computed assertions
```

## The full cycle

```
discover ──▶ brief ──▶ generate ──▶ check ──▶ queue ──▶ build ──▶ human merge
(scout.cjs)  briefs/   (generate-   (check-    queue/    (build-   ao/build-<slug>
                       spec.cjs)    spec.sh)   + wih:     queue.sh)  merged by a
                                    + wih:     pipeline-  + wih:     human after
                                    pipeline-  queue      pipeline-  review
                                    queue                 builds
```

```bash
node docs/pipeline/bin/scout.cjs && node docs/pipeline/bin/generate-spec.cjs && bash docs/pipeline/bin/check-spec.sh
bash docs/pipeline/bin/build-queue.sh --all        # build everything queued (blocking watch)
bash docs/pipeline/bin/build-queue.sh --all --no-wait   # spawn all, watch manually
bash docs/pipeline/bin/build-queue.sh              # list queue contents
bash docs/pipeline/bin/build-queue.sh <slug> ...   # build specific queued specs
```

## Brief format (parsed by the generator)

Both the LLM path and the `TODO(agent)` fallback in the scout emit exactly
this structure; an agent completes the TODO sections before generating:

```markdown
# <title>

- Source: <source>
- URL: <url>
- Relevance score: <0..1>

## What it is

One paragraph.

## Mechanism

- Fact about how it works internally

## Integration surface

- <repo path or subsystem>: <what would change>

## Requirements seed

- WHEN <trigger>, THE SYSTEM SHALL <observable behavior>
- WHEN <trigger>, THE SYSTEM SHALL <observable behavior>

## Excluded            ← optional, merged into the spec's Out of scope

- <anything explicitly out>
```

## Running the scout

```bash
node docs/pipeline/bin/scout.cjs
```

Prerequisites: rails must be reachable AND writable. The scout runs
`bin/rails-ensure.sh` first; if nothing healthy is on port 8013 it starts the
dev API (`dev/scripts/start-api.sh`, which sets `ALLTERNIT_LOCAL_DEV_BYPASS=1`
— same as `make api`). If the port is held by an instance that rejects
pipeline writes (e.g. the packaged app), it exits non-zero naming the blocker:
run `make stop && make api` or stop the packaged app. There is no fallback.

If `KIMI_API_KEY` is set, briefs are drafted by `callKimi()` from the repo's
pipeline lib; otherwise a `TODO(agent)` template is written per brief — an
agent completes the structured sections before the generator will accept the
brief (raw TODO placeholders are rejected by the strict parser).

## Running the generator

```bash
# One brief (regenerates unconditionally):
node docs/pipeline/bin/generate-spec.cjs docs/pipeline/briefs/<slug>.md

# All new/changed briefs (skips up-to-date via specs/.generated.json):
node docs/pipeline/bin/generate-spec.cjs
```

Emits `docs/pipeline/specs/<slug>.md` (Context / R1..Rn / Out of scope / Gherkin
acceptance). Deterministic: same brief in → byte-identical spec out. A brief
missing `## Requirements seed` or with a non-`WHEN…THE SYSTEM SHALL` seed
bullet is rejected with an error naming the brief and the offending line —
never improvised.

## Testing

```bash
node docs/pipeline/bin/scout-test.cjs
node docs/pipeline/bin/generate-spec-test.cjs
bash docs/pipeline/bin/check-spec-test.sh
bash docs/pipeline/bin/build-queue-test.sh
bash docs/pipeline/bin/taste-test.sh
bash docs/pipeline/bin/wiki-test.sh
bash docs/pipeline/bin/contract-test.sh
bash docs/pipeline/bin/learn-test.sh
bash docs/pipeline/bin/proposals-test.sh
bash docs/pipeline/bin/metrics-test.sh
```

Offline: stubs the source fetch and the rails announce; verifies the three
offline-testable Gherkin scenarios from `.steering/spec.md` (fresh run caps at
5 briefs, re-run drains the backlog without duplicating, rails outage aborts
before any brief). The generator test uses fixture briefs (valid + two
malformed) and checks EARS preservation, rejection errors, byte-identical
regeneration, and the manifest. The checker test stubs the consult
(`SPEC_CHECK_CMD`) and curl (PATH shim) and verifies READY/NEEDS-WORK/STALLED
handling, the memory lesson POST, and the advisory memory-failure path.
The build-queue test PATH-shims `ao-spawn`/`ao-send`/`ao-watch`/`curl` and
verifies the rails-ensure abort, task-file generation, session names,
`built`/`failed` recording, the rails announcements, announce-failure
retry (announce-only, no re-spawn), ao-send-failure state, built-skip on
re-run, `--no-wait`, and the empty-queue path. The taste test stubs curl and
verifies per-source metadata, trust-rule tiers, ledger idempotency, outcome
recording + posting, `[stale]` marking of old precedents, and the
memory-down advisory paths. The wiki test stubs curl and the scout deps and
verifies idea/pain → unverified candidates, context pages yielding none, the
C2-R1 injection page (candidate marked unverified, wiki byte-identical, no
writes outside `candidates/` + the ledger), dismissal recording, and scout
suppression inside/outside the 14-day window; it runs the contract test as
its final block. The contract test validates the golden fixtures in
`taste/golden/`, regenerates the fixture spec (byte-identical modulo
`produced_at`), and validates the live scout/check-spec producers against the
same frontmatter contract. The learning-loop test stubs the consults
(`STEER_CONSULT_CMD`/`SPEC_CHECK_CMD`/`LEARN_CONSULT_CMD`) and curl and
verifies capture-on-verdict (gate, check-spec, record-outcome, dismiss),
reflection distillation + watermark idempotency + advisory failure, playbook
inclusion in both consult assemblies, and `[stale]` marking / the 4KB cap;
it also covers M3 brain persistence and M2-R1 proposal generation. The
proposal audit test fixtures a temp git repo and stubs the auditor consult
and curl; it verifies ADOPT application + slug-referencing commit + outcome
linkage (data and code targets), REVISE recording without application,
REJECT no-op + memory ingest, consult-failure skip, verdict merge semantics,
and re-run idempotency. The metrics test runs hand-computed fixture logs
(consults/events/outcomes/git timestamps) through `metrics.sh` and verifies
the R1 rates, per-week `insufficient_data` honesty on thin data (R3),
history append-on-change idempotency (R4), and the memory-down advisory
path (R2).

## Layout

- `bin/`, `spec-rubric.md`, `proposal-rubric.md`, `taste/trust-rules.json`,
  `taste/golden/`, `learn/reflect-prompt.md`, `playbook.md`, `proposals/` —
  committed code, prompts, trust config, contract fixtures, the learned
  playbook, and the proposal audit trail
- `briefs/`, `specs/`, `queue/`, `seen.json`, `errors.log`, `verdicts.json`,
  `builds/`, `builds.json`, `taste/ingested.json`, `outcomes.jsonl`,
  `candidates/`, `dismissals.json`, `learn/events.jsonl`, `learn/watermark`,
  `proposals/verdicts.json`, `metrics/latest.json`, `metrics/latest.md`,
  `metrics/history.jsonl` —
  gitignored runtime artifacts (multi-machine sync is via rails asset refs,
  not git)
