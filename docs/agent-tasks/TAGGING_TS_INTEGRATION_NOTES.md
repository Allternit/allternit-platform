---
status: done
files_changed:
  - surfaces/ai.allternit.com/src/lib/tags/useTags.ts
  - surfaces/ai.allternit.com/src/components/tagging/TagPicker.tsx
  - surfaces/ai.allternit.com/src/views/tags/TagManagerView.tsx
  - surfaces/ai.allternit.com/src/views/agent-view/main/AgentGalleryGrid.tsx
deviations: []
remaining: []
---

# Tagging Subsystem — Surface Integration Notes

## What changed

1. **Created `useTags` lifecycle hook** (`src/lib/tags/useTags.ts`)
   - Calls `useTagStore.getState().loadTags()` once on mount (guarded by `isHydrated`/`isLoading` so it does not spam the API).
   - Returns `{ tags, taggings, isLoading, error }` for components to subscribe to.

2. **Wired the hook into the UI**
   - `TagPicker` now uses `useTags()` instead of directly selecting `tags`/`taggings`, ensuring data is fetched when the picker opens.
   - `TagManagerView` uses `useTags()` for the same lifecycle behavior.
   - `AgentGalleryGrid` calls `useTags()` so that `getTagsForTarget` consumers have tag data loaded before filtering gallery cards.

3. **Made all store action callers async-aware**
   - `TagPicker`: `handleCreate`, add-tag `onClick`, and remove-tag `onRemove` are now `async` and `await` `createTag`/`addTagging`/`removeTagging`.
   - `TagManagerView`: `handleSave` awaits `createTag`/`updateTag`, and tag deletion goes through a new `handleDeleteTag` helper that awaits `deleteTag`.
   - Added a local `isSubmitting` flag to each component and disabled relevant buttons while an action is in flight.

4. **Surfaced errors**
   - Both `TagPicker` and `TagManagerView` pull `error` from the store and render it as red text.
   - `resetError()` is called at the start of each mutating action so stale errors are cleared before the next attempt.

5. **No Rust backend changes** and no new dependencies.

## Typecheck result

- Ran `bun typecheck` from `surfaces/ai.allternit.com`.
- The touched files (`useTags.ts`, `TagPicker.tsx`, `TagManagerView.tsx`, `AgentGalleryGrid.tsx`) produced **no new errors**.
- The surface still reports a large number of **pre-existing type errors** in unrelated modules (`ai-elements`, `agent.types`, plugins, `tokenlens`, etc.). These are out of scope for the tagging integration task and were left untouched.

## Remaining work

None for this task. The tagging surface is now fully API-backed and the async lifecycle is wired into its callers.
