# Live group room brief — merge attestation

Eoj approved the room brief design and explicitly requested a GitHub merge. UI PR [allternit-ai #766](https://github.com/Allternit/allternit-ai/pull/766) merged at `2b32c9ebed28fd08555ac5f674133754bf95a093` on 2026-10-23 03:51 UTC. Implementation commit: `b2533a3004b4f90279ded0d9443ea00c49bd7862`.

## Behavior

The group center has Now (request, latest quote, live speaker and handoff), Moments (up to 12 real recent bot replies with explicit paced playback and reading pauses), and Shared (up to eight explicit HTTPS media references with Load, Retry and original links). No generated task status or video autoplay. Reduced motion removes playback and travel. Desktop, web phone and PWA share the component; phone uses a compact cast row and reachable controls. Sending/scrolling docks into the members pill so chat takes the screen; tapping restores the brief.

This companion change documents the behavior in the user guide and a new architecture guide, adds navigation and records the dependency-map decision. No API, runtime or shipping contract changed.

## Verification

- 18 focused Vitest tests passed on the latest-main base, covering group model, brief interactions, media, reduced motion and group-thread integration/docking.
- Targeted ESLint and git diff --check passed.
- Real RoomBrief artifact with sample data reviewed frame by frame on phone, dark mode, reduced motion and interrupted transitions. The preview is not a deployed-app smoke test.
- Docs: 582 navigation entries, 577 pages, zero link problems.
- Dependency map: valid 33 products, 65 features, 13 journeys.
- GitHub reported UI PR mergeable/clean and no attached checks. No new local production build or typecheck was run, per Eoj's standing instruction.

## Review and release boundary

The normal Factory/Kimi commit consult returned an empty answer, which the hook classified STEER without findings. A real independent Claude review through Factory `reviewer@group-room-review` returned APPROVE for UI commit b2533a30 and the scoped docs diff. Scratch steering files are not part of this PR. Factory plan: dag_10014; UI node n_8598; docs/cleanup node n_4092.

Merging UI main triggers the existing ai.allternit.com Cloudflare Git deployment. Production deployment success is not attested here. Desktop has not been rebuilt or installed, and the separate manual m.allternit.com PWA deployment has not run. This session's authorization was to merge to GitHub.
