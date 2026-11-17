# Settings contract (Primitives Wave D, D2)

`definitions.json` is the one list of settings the `settings` tool can read or
change, and the risk class of each. It is shared by:

- the gizzi runtime (`cmd/gizzi-code/src/runtime/primitives/settings.ts`, the
  `settings` builtin and the Ink SettingsTool), which reads it on demand, never
  in every turn's prompt (decision 14);
- allternit-api (`agent_settings_tools.rs`), which enforces the risk gate;
- the clients' settings appliers (ai `lib/settings/agent-settings-apply.ts`,
  iOS `AgentSettingsApplier.swift`) for per-device keys.

## Classes (decision 12)

| Class | Who applies it | Tool outcome |
|---|---|---|
| `safe` | the runtime (account and runtime keys), or the client that sent the turn (device keys) | `applied` or `apply_on_device` |
| `confirm` | only the user's own tap on the card, under their sign-in; refused for runs an automation started | `needs_confirmation` |
| `locked` | nobody from a chat; the card links to Settings | `locked` |

An unknown key, or a value the key does not take, comes back `invalid`. The
server enforces the class: a `confirm` or `locked` key is never applied by a
runtime call, even under allow-all permissions.

## Fields

- `key`: dotted name the model uses. `notifications.*` stands for each
  notification toggle.
- `class`: `safe`, `confirm` or `locked`.
- `scope`: `account` (allternit-api), `device` (applied by the client that sent
  the turn, once per tool-call id; other devices show a one-tap Apply, decision
  13) or `runtime` (gizzi config on the computer running the session).
- `type`, `values`: what a valid value is.
- `settings_path`: the Settings screen the card opens for `confirm` and `locked`.
- `store`: where each surface keeps it today (from the D0 facts). Writes go
  through the existing preference functions (`/agent-preferences`,
  `/cowork-preferences`); no new storage.

## Changing it

Add or move a key here first, then the server gate and the appliers. A key
moves to a less strict class only with Eoj's sign-off. W2 skeleton: lane S
fills the remaining `store` entries and values.
