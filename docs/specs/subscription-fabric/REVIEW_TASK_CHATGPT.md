# Review Task — Subscription Capability Fabric (Implementation Reviewer)

You are reviewing an architecture spec for a new Allternit subsystem. You are the **implementation reviewer**. Your output feeds a synthesis that produces a hardened spec + implementation plan.

## Read first

1. `docs/specs/subscription-fabric/SPEC.md` — the full spec (this is the subject of your review).
2. `REPO_STRUCTURE.md` (repo root) and the repo `AGENTS.md` — to ground recommendations in this monorepo's conventions.
3. Explore the repo as needed. It is Rust + TypeScript (pnpm workspace + cargo workspace). Long-running daemons live in `services/`, internal TS libs in `platform/packages/`, surfaces in `surfaces/`. Playwright is already used (see `scripts/e2e/bot/bot-e2e-playwright.cjs` and `scripts/e2e/bot/bot-e2e-web.cjs`). The local API gateway is `cmd/allternit-api` (Rust; installed instance owns port 8013, dev default 18013).

## Context (do not relitigate)

- The user owns every subscription account that will be connected. ToS risk is accepted by the user — do not argue ToS. Cover **operational risk** instead: UI brittleness, selector drift, session expiry, detection avoidance, graceful degradation.
- Target providers: ChatGPT, Claude, Kimi web apps first, with cheap adapter extensibility (Gemini and similar chat-subscription providers later).
- The implementation will live inside THIS monorepo.

## Your review focus (implementation)

1. **Where it lives** — recommend the exact home in this monorepo (e.g. `services/subscription-fabric/`? a crate + TS package split?) with reasoning from existing service layouts. Look at 2–3 existing entries in `services/` and describe the pattern (manifest, entrypoint, port, config).
2. **Stack decision** — Rust vs TypeScript (Node/Bun) for the Subscription Gateway + provider workers, given: Playwright UI bridges, SQLite state, SSE/streaming to callers, this repo's existing tooling. Pick one primary stack and justify; say what (if anything) should be in the other language.
3. **Reuse map** — what already exists in this repo that the fabric should reuse instead of rebuilding: browser automation harnesses, task/queue patterns, artifact storage, bot/thread models, observability/event-log plumbing, config conventions, port allocation. Cite real paths you verified.
4. **Concrete file layout** — a full directory tree for the new subsystem following repo conventions, with one-line purpose per directory.
5. **Phased build order** — refine spec §27/§41 into concrete phases sized for agent-executed sessions (each phase reviewable in a small diff), with per-phase verification commands (typecheck/tests/curl smoke). Include the ChatGPT-chat vertical slice, the ChatGPT image slice, the Kimi artifact slices, and the adapter-extension point proven by adding a mock "Gemini-shaped" adapter.
6. **Testing strategy** — fixture HTML pages + mocked provider interfaces; where tests live in this repo and how they're run.
7. **Spec gaps that block implementation** — anything underspecified that an engineer would hit in week 1 (auth/session bootstrap details, download interception, streaming detection, task cancellation semantics, idempotency).

## Deliverable (REQUIRED)

Write exactly one file: `docs/specs/subscription-fabric/REVIEW_CHATGPT.md` starting with YAML frontmatter:

```yaml
---
status: done
reviewer: chatgpt
summary: <one sentence>
---
```

Then sections: `## Verdict`, `## Recommended home and stack`, `## Reuse map` (verified repo paths), `## File layout`, `## Phased build order` (with verification commands), `## Testing strategy`, `## Week-1 blockers in the spec`, `## Open questions for the human`.

## Constraints

- Review-only. Do NOT modify any existing file. Do NOT create any file other than your review file. No git operations. No builds, no dev servers, no dependency installs.
- Every repo claim must be a path you actually opened — no guessing.
