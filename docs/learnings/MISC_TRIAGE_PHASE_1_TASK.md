# Misc Sections Triage — Phase 1

## Scope

Re-investigate all remaining open items in `docs/learnings/SURFACE_AUDIT_SESSION_PROGRESS.md` that are NOT covered by the other triage agents. Do NOT write implementation code. This phase is verification + classification only.

### Items to verify

Core Chat/Home (rows 1–2):
1. Projects (GAP → gizzi-code)
2. Artifacts Library (PARTIAL → upgrade)

Marketplace/Plugins (rows 88–91):
3. Marketplace (top-level) (PARTIAL → iOS)
4. Plugin Registry / Plugin Marketplace (PARTIAL → iOS)
5. Team Skills panel (GAP → iOS)

Products/Discovery (rows 92–96):
7. Products Discovery (GAP → iOS, gizzi-code)
8. A://Labs (GAP → iOS, gizzi-code)
9. Udemy Catalog (GAP → iOS, gizzi-code)
10. Discovery Feed (GAP → iOS, gizzi-code)
11. Research tab/panel (PARTIAL → iOS)

Mail/Knowledge (rows 97–99):
12. Mail Monitor (GAP → iOS)
13. Documents (office-file I/O) (GAP → iOS)
14. Knowledge (PARTIAL → iOS)

Onboarding/Account (rows 100–110, excluding Device Pairing already shipped):
15. Settings (umbrella/shell) (PARTIAL → upgrade)
16. Settings > Account (PARTIAL → gizzi-code)
17. Settings > Platform (PARTIAL → iOS)
18. Settings > Products (GAP → iOS)
19. Settings > Infrastructure (GAP → iOS, gizzi-code)
20. Settings > Customize (PARTIAL → upgrade)
21. Organization Access panel (GAP → iOS, gizzi-code)
22. Compute Billing panel (GAP → iOS)
23. Enterprise BYOC panel (GAP → iOS, gizzi-code)
24. Model Management view (PARTIAL → upgrade)

AllternitOS (row 111):
25. AllternitOS (GAP → gizzi-code)

Playground/QA (row 112):
26. Playground (PARTIAL → iOS)

Empty stubs (row 113):
27. `views/gizzi`, `components/mesh`, `lib/mesh-network` (PARTIAL → upgrade)

Desktop-only (rows 114–115):
28. Local runtime discovery (PARTIAL → upgrade)
29. Local Python execution (PARTIAL → upgrade)

gizzi-code-only (rows 116–120):
30. `github ...` GitHub Actions agent bot (GAP → iOS, gizzi-code)
31. Local VM management (GAP → gizzi-code)
32. Teleport / remote dev environments (GAP → gizzi-code)
33. Slack app install (GAP → gizzi-code)
34. Theme switching (`/theme`) (PARTIAL → upgrade)

## Investigation method

For each item:
1. Search web (`surfaces/ai.allternit.com/src/`), iOS (`surfaces/allternit-mobile/ios/`), and gizzi-code (`cmd/gizzi-code/src/`) for the named component/feature.
2. Check for implementations, stubs, dead routes, or missing surfaces.
3. Classify each item exactly as one of:
   - `REAL` — confirmed live gap.
   - `STALE` — already exists, renamed, dead code, or duplicate.
   - `DEFER` — real but out of scope for this phase.

## Deliverable

Write `docs/learnings/MISC_TRIAGE_PHASE_1_NOTES.md` with this exact YAML frontmatter:

```yaml
status: done
files_changed: []
deviations: []
remaining: []
```

Then prose with a subsection for each of the 34 items containing classification, evidence (file paths), and recommended next action.

Also update `docs/learnings/SURFACE_AUDIT_SESSION_PROGRESS.md` in-place: set the `Status` column for each of these rows to `[x]` and the `Finding` column to your classification + brief evidence.

## Constraints

- Read-only investigation. Do not modify source files.
- Do not run builds, typechecks, or dev servers.
- Do not commit or push.
- Be concise; cite exact file paths.

When finished, `docs/learnings/MISC_TRIAGE_PHASE_1_NOTES.md` must exist. That file existing = done.
