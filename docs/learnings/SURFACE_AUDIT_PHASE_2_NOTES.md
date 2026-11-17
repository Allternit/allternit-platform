---
status: done
files_changed:
  - docs/learnings/SURFACE_AUDIT_FINAL_REPORT.md
deviations:
  - "Three items were seriously considered as new Step 1 rows and deliberately excluded because they turned out to already be represented in an existing Phase 1 row under different wording, per the task's own instruction that same-concept-different-words counts as covered: (1) iOS's Authentication/Clerk sign-in flow — already assessed within Phase 1's \"Settings > Account (Sign-in, Org & Access...)\" row; (2) gizzi-code's `agent` command (select/list active agent mode) — folded into the already-covered Agent Hub row as part of the same \"manage agents\" capability area; (3) gizzi-code's Session sharing/ShareNext subsystem — the same underlying `export`/`import`-based mechanism as the already-covered \"Cowork Exports view\" row, just described at a different level of detail in RAW INVENTORY 3. These are documented explicitly in the final report's \"Considered but already covered\" list rather than silently dropped."
  - "gizzi-code's 13 built-in domain skill bundles (data-sql, data-visualization, engineering-code, engineering-incident, finance-analysis, hr-recruiting, legal-contracts, legal-nda, marketing-content, operations-runbooks, product-management, search-knowledge, security-compliance) were not each given their own Step 1 row. Two (search-knowledge, security-compliance) are already referenced in existing Phase 1 rows; the other 11 are specific *content* within the general \"browse/install skills\" capability already covered by the Skills Registry / Team Skills panel / Plugin Registry rows, consistent with how Phase 1 itself never itemized each of Web's 480 individual built-in plugins. Noted explicitly in the final report rather than silently omitted."
  - "A handful of Step 1 gizzi-code items (`attach`, `pr`, `db`, `status`, `web`, `allternit`, shell completion, `uninstall`, `upgrade`, `serve`, `generate`, `acp`) are terse, one-line INTENTIONALLY SURFACE-SPECIFIC rows rather than deeply-reasoned ones — this mirrors Phase 1's own precedent for its Desktop-only section, which had a similar density of short, clearly-reasoned surface-specific rows for CLI/Electron plumbing that doesn't need extended discussion."
remaining:
  - "Same 7 UNCLEAR items carried forward from Phase 1 (5) plus 2 new ones from Step 1 (Attachments, `ac` inter-agent messaging) — full list with exactly what to check for each is in the final report's \"UNCLEAR items\" section, not repeated here."
---

## Summary

Step 1 went through RAW INVENTORY 2 (iOS) and RAW INVENTORY 3
(gizzi-code) in `docs/learnings/SURFACE_AUDIT_MAP.md` item by item, checking each
against every row in the approved Phase 1 matrix (including cases where
the same concept is referenced from a different surface's column under
different wording). This found **21 new rows** with no Web/Desktop
equivalent anchoring them in Phase 1:

- **3 from iOS**: Local Response Notifications, Attachments
  (camera/photos/files composer staging), In-app Safari browsing
  (`SFSafariViewController`).
- **18 from gizzi-code**: mostly small CLI/dev-lifecycle commands the
  source inventory itself already tags "no" / not GUI-shaped (`attach`,
  `generate`, `acp`, `upgrade`, `uninstall`, `serve`, `web`, `status`,
  `pr`, `db`, `allternit`, shell completion), plus five substantive ones —
  `github` integration setup, `ac` inter-agent messaging, Local VM
  management, Teleport/remote dev environments (as a standalone
  capability distinct from the already-covered Checkpointing row), Slack
  app install, and Theme switching.

Three additional items were seriously considered and explicitly excluded
as already-covered-under-different-wording rather than silently dropped
(iOS Authentication, gizzi-code's `agent` command, gizzi-code's Session
sharing subsystem) — see the `deviations` above and the final report's
"Considered but already covered" list for the reasoning on each.

**Final combined row count: 192** (171 Phase 1 + 21 Step 1), written to
`docs/learnings/SURFACE_AUDIT_FINAL_REPORT.md` along with summary statistics, a
prioritized GAP list, and an UNCLEAR-items checklist.

## GAP count in the prioritized summary

**83 total GAP items** (79 from Phase 1 + 4 new from Step 1: `github`
integration, Local VM management, Teleport/remote dev environments, Slack
app install). Grouped in the final report into:

- **(a) Core product gaps a regular user would notice** — 8 items
  (Changeset Review, Device Pairing panel, Automation Tasks, the Cowork
  sub-view cluster, Documents, Mail Monitor, ACI/browser automation on
  gizzi-code, Office Add-ins).
- **(b) Capabilities engineers/power-users would want** — 7 groupings
  (the DAG-suite cluster missing from gizzi-code, Code-mode IDE tooling
  missing from iOS, `github` integration, Local VM management, Teleport,
  infra/ops panels, Skills Registry).
- **(c) Lower-priority/edge-case gaps** — 9 groupings (Design Mode's
  non-canvas tabs, Form Surfaces, discovery/marketing features,
  admin/billing panels, Team Skills panel, Cowork sub-config panels,
  AllternitOS, Mini-apps/Slack install).

Full classification breakdown across all 192 items: **83 GAP (43%), 38
PARTIAL (20%), 51 INTENTIONALLY SURFACE-SPECIFIC (27%), 13 FULL PARITY
(7%), 7 UNCLEAR (4%)** — counts verified by grepping the classification
column of both the Phase 1 matrix and the final report's Section 1B
independently rather than hand-tallying.

Made no code changes anywhere, per the phase task's scope. This was the
last phase.
