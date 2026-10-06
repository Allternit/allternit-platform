# Task: Settings UI Parity — Phase C ONLY

Phase A+B is done and reviewed. Now implement Phase C of `docs/learnings/CLAUDE_DESKTOP_SETTINGS_PARITY_MAP.md` using the primitives you created in `src/components/settings/`.

Note: a reviewer made one fix to your Phase A work — in SettingsView.tsx the content pane is now a non-scrolling `relative` wrapper holding the absolute `×` button, with an inner `h-full overflow-y-auto` div doing the scrolling (so the × stays pinned). Preserve that structure.

## Scope (all in `surfaces/ai.allternit.com/src/views/settings/` unless noted)

1. **Migrate every remaining section** off `ToggleItem`/card layouts onto `SettingsRow`/`Toggle`/`SectionHeading`: models, api-keys, shortcuts, permissions, gizziio-code, cowork, extensions, billing, usage, diagnostics, about, signin. (infrastructure/environment/security/agents/vps render external components — leave their internals alone this phase.) Then delete `ToggleItem` once nothing uses it.
2. **Account (signin) section**: add rows per the map — "Log out of all devices" (quiet bordered button), Organization/user ID as `MonoChip`, "Active sessions" table via `SettingsTable` + `SettingsTableChip` ("Current") using `usePlatformSessions` from `@/lib/platform-auth-client`, and a "Trusted devices" table with an inline empty message when there are none.
3. **Usage section**: thin progress-bar rows (gray track, right-aligned "N% used"), grouped "Current session" / "Weekly limits" with `SectionHeading`, "Last updated + refresh" row. Keep `ResourceUsageDashboard` available below as a details block.
4. **New Privacy section**: add to `SETTINGS_NAV_ITEMS` (group: account, after usage/billing). Rows: two link-style rows ("How we protect your data", "How we use your data" — chevron rows, can be no-ops), Preferences toggles (location metadata, improve-models), "Your data" group with Export data / Shared chats Manage / Memory preferences Manage buttons (wire to existing handlers where they exist, else disabled with a `title="Not wired yet"`).
5. **Gizzi Code section**: restructure per map — General toggles, "Code appearance" with two theme selects and a side-by-side light/dark code diff preview (static sample, real diff colors), "Browser" group (browser tools toggle, persist-sessions select), "Pull requests" group (branch prefix text input, create-PRs-automatically toggle, autofix toggle).
6. **Cowork section**: Dispatch toggle with `Badge`>Beta, files-location row (path link + "Use recommended" warning-styled button + "Change" button), Trusted folders row with "Manage" button, Global instructions row with "Edit" button.
7. **List panels — Skills, Connectors, Plugins**: add all three to `SETTINGS_NAV_ITEMS` under a new `customize` group ("Customize"). Each renders a `PanelHeader` (title + search icon + "Browse" + "Add ▾" buttons) and either a `SettingsTable` (name / last updated / author) or an `EmptyState` with one CTA. For Connectors, reuse the data/content from `src/views/cowork/ConnectorSettingsPanel.tsx` if practical; otherwise render an EmptyState. Use `SkeletonRow` while any of these load.
8. **Beta badges** on beta-flagged rows; restyle remaining loud buttons to the quiet bordered secondary style; destructive actions get the muted gray treatment.

## Constraints (same as before)
- NEVER run builds/typechecks/dev servers (no tsc, npm/pnpm/bun build, cargo).
- No git operations.
- Match existing idiom: Tailwind + CSS vars, `cn()`, phosphor icons, named exports.
- Do not touch files outside the settings view + settings.config + components/settings, except adding nav entries. Do NOT modify anything else in src/lib.
- Keep `SETTINGS_NAV_ITEMS`/`SETTINGS_NAV_GROUPS` as the single source of truth.

## Deliverable
When finished, write `docs/learnings/SETTINGS_PARITY_PHASE_C_NOTES.md`: files changed, per-section summary, deviations + why, anything left for Phase D. That file existing = done.
