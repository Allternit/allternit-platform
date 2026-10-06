# Settings UI Parity — Phase D Notes

## Summary

Phase D completed the final settings parity pass:

- Extracted the agent operations dashboard from `SettingsView.tsx` into `surfaces/ai.allternit.com/src/views/settings/AgentOpsPanel.tsx`.
- Extracted the security dashboard from `SettingsView.tsx` into `surfaces/ai.allternit.com/src/views/settings/SecurityPanel.tsx`.
- Kept `SettingsView.tsx` as the settings shell and section router, with extracted panel imports for `AgentOpsPanel` and `SecurityPanel`.
- Added persisted settings state through `surfaces/ai.allternit.com/src/hooks/useSettingsState.ts`.
- Completed the external panel idiom sweep for `InfrastructureSettings.tsx`, `EnvironmentSettings.tsx`, `ServiceUrlSettings.tsx`, and `VPSConnectionsPanel.tsx`.

## Files changed

- `surfaces/ai.allternit.com/src/views/settings/SettingsView.tsx`
- `surfaces/ai.allternit.com/src/views/settings/AgentOpsPanel.tsx`
- `surfaces/ai.allternit.com/src/views/settings/SecurityPanel.tsx`
- `surfaces/ai.allternit.com/src/views/settings/InfrastructureSettings.tsx`
- `surfaces/ai.allternit.com/src/views/settings/EnvironmentSettings.tsx`
- `surfaces/ai.allternit.com/src/views/settings/ServiceUrlSettings.tsx`
- `surfaces/ai.allternit.com/src/views/settings/VPSConnectionsPanel.tsx`
- `surfaces/ai.allternit.com/src/components/settings/buttonStyles.ts`
- `surfaces/ai.allternit.com/src/hooks/useSettingsState.ts`
- `docs/learnings/SETTINGS_PARITY_PHASE_D_NOTES.md`

## SettingsView extraction

`SettingsView.tsx` was reduced from 2,369 lines before Phase D to 1,187 lines after extraction.

Moved to `AgentOpsPanel.tsx`:

- Evaluation, factory, and garbage-collection dashboard internals.
- Agent operations API helper object.
- Agent operations local types and subcomponents used only by that panel.

Moved to `SecurityPanel.tsx`:

- Security tabs and dashboard internals.
- Security policy, approval, purpose-binding, and compliance render logic.
- Security-only dashboard stat helpers.

`SettingsView.tsx` now imports the extracted panels and routes:

- `security` to `<SecurityPanel />`
- `agents` to `<AgentOpsPanel />`

The `renderContent` switch was checked against every id in `SETTINGS_NAV_ITEMS`; every configured id is covered.

## Persistence approach

Phase D added `useSettingsState(key, initial)` as a small localStorage-backed replacement for `useState`.

Storage keys use:

```text
allternit.settings.v1.<key>
```

The hook JSON-serializes values, initializes from localStorage when available, and falls back to in-memory state if storage is unavailable. This was chosen because the migrated settings are local UI preferences and do not belong to external service-owned state. Existing external components and backend-owned settings were left alone.

Persisted settings now cover migrated general, appearance, privacy, Gizzi Code, cowork, and extensions state.

## Pickup work completed

`InfrastructureSettings.tsx` had a partially completed idiom sweep. The pickup pass finished it by:

- Replacing remaining large `h2`/`h3` section headings with `SectionHeading`.
- Replacing ad-hoc spinner loading states for templates and nodes with `SkeletonRow`.
- Updating the nodes empty state to the shared `EmptyState` API.
- Replacing remaining accent-filled action buttons with `QUIET_BUTTON_CLASS`.
- Using `DESTRUCTIVE_BUTTON_CLASS` for the deployment cancel action.
- Removing the now-unused local `LoadingState` helper.

The parallel-swept panels were sanity-checked and adjusted:

- `EnvironmentSettings.tsx`: headings normalized to sentence case.
- `ServiceUrlSettings.tsx`: headings normalized, Save button moved to `QUIET_BUTTON_CLASS`.
- `VPSConnectionsPanel.tsx`: Add Connection button moved to `QUIET_BUTTON_CLASS`; stale `router` dependency fixed to `navigate`.

## Verification

Verified by reading and targeted text scans only, per task constraints.

Confirmed:

- `AgentOpsPanel` and `SecurityPanel` imports are present in `SettingsView.tsx`.
- `renderContent` covers all ids from `SETTINGS_NAV_ITEMS`.
- No targeted leftover `LoadingState`, old nodes `EmptyState` props, stale `router`, or `SAND` usage remains in the swept panels.
- `SettingsView.tsx` current line count is 1,187.

## Deviations

- No builds, typechecks, or dev servers were run, per the task constraints.
- No logic/API restructuring was done in the external panels; changes were limited to shared primitives, shared button classes, and the one stale dependency found during sanity-checking.

## Remaining

No known Phase D remainder.
