# Settings contract (Primitives Wave D, D2)

`definitions.json` is the one list of settings the `settings` tool can read or
change, and the risk class of each. It is shared by:

- the gizzi runtime (`cmd/gizzi-code/src/runtime/primitives/settings.ts`, the
  `settings` builtin and the Ink SettingsTool), which reads it on demand, never
  in every turn's prompt (decision 14);
- allternit-api (`agent_settings_tools.rs`), which enforces the risk gate;
- the clients' settings appliers (ai `lib/settings/agent-settings-apply.ts`,
  iOS `AgentSettingsApplier.swift`, the Ink SettingsTool) for device and
  runtime keys.

## Classes (decision 12)

| Class | Who applies it | Tool outcome |
|---|---|---|
| `safe` | allternit-api (account keys), or the client that sent the turn (device and runtime keys) | `applied` or `apply_on_device` |
| `confirm` | only the user's own tap on the card, under their sign-in; refused (`invalid`) for runs an automation started | `needs_confirmation` |
| `locked` | nobody from a chat; the card links to Settings | `locked` |

An unknown key, or a value the key does not take, comes back `invalid`. The
server enforces the class: a `confirm` or `locked` key is never applied by a
runtime call, even under allow-all permissions. allternit-api has no code path
that writes a `confirm` or `locked` key; its tests check that.

## Routes

- `POST /api/v1/bot-routing/settings/read` `{sessionId, keys?}` →
  `{settings: [definition + value?], unknown: [key]}`. `value` is present for
  account keys the server can read; device and runtime keys have none.
- `POST /api/v1/bot-routing/settings/write`
  `{sessionId, toolCallId, changes ≤5 [{key, value}], automationRun?}` →
  `{toolCallId, changes: [{key, outcome, scope?, value?, previous?, reason?, settings_path?}]}`.
  `automationRun` is set by the runtime (never the model) when a scheduled run
  started the turn; the server also checks its own scheduler. `previous` is the
  value before the write when the server knows it (account keys); device and
  runtime appliers use their own stored value for Undo.

Whose settings they are comes from the session id (`resolve_owner`), never
from the model.

## Fields

- `key`: dotted name the model uses.
- `class`: `safe`, `confirm` or `locked`.
- `scope`: `account` (allternit-api, the server), `device` (applied by the
  client that sent the turn, once per tool-call id; other devices show a
  one-tap Apply, decision 13) or `runtime` (Gizzi Code's config, applied by the
  Gizzi Code client the same way as device keys).
- `type`: `boolean`, `string`, `enum` (one of `values`) or `list` (strings).
- `max`: longest string, or most list items.
- `label`, `description`: what the card and the model see.
- `settings_path`: the Settings screen (`/settings/<section>`) the card opens.
- `store`: where each surface keeps it today (D0 facts). Writes go through the
  existing preference handlers (`PUT /agent-preferences`,
  `PUT /cowork-preferences`); no new storage. Appliers that have no store for a
  key fall back to "Open Settings".

## Changing it

Add or move a key here first, then the server gate (`account_field` in
`agent_settings_tools.rs` for a new safe account key) and the appliers. A key
moves to a less strict class only with Eoj's sign-off.
