# Contract: Phone App Maps, 2026-11-18

Status: draft, from `/Users/joe/.claude/plans/temporal-orbiting-axolotl.md` (plan step 1). Steps 2 to 13 build against this file. Real-device behaviour is "not checked" until run. Anything marked "to confirm in step N" was not confirmed in code when this was written.

Key words: MUST, MUST NOT, MAY are normative.

An App Map is a per-app graph of screens. The phone agent replays known paths with no model call and uses the model only on unknown screens. It adds no second phone app and copies no PhoneGo code.

Paths: `P/` is the platform repo (`allternit`). Rust files are in `P/cmd/allternit-api/src/` unless stated.

## 1. `phone.read_ui` additive result

Today the result is `{ ok, text }` (`phone-tools.ts` `readUi`). It gains three fields. `text` is unchanged byte for byte.

```json
{
  "ok": true,
  "text": "<unchanged>",
  "app": "com.android.settings",
  "activity": "com.android.settings.Settings$WifiSettingsActivity",
  "nodes": [
    { "index": 0, "class": "android.widget.TextView", "label": "Wi-Fi", "resource_id": "android:id/title",
      "bounds": [0, 210, 1080, 330], "clickable": true, "scrollable": false,
      "checkable": false, "editable": false, "password": false, "focused": false }
  ]
}
```

| Field | Type | Default | Notes |
|---|---|---|---|
| `app` | string | `""` | Android package, or iOS bundle id when known. `""` = unknown. |
| `activity` | string or null | `null` | Android activity class of the focused window. iPhone: always `null`. |
| `app_version` | string or null | `null` | Android `versionName`. Only when the caller passes `with_version: true` (one extra adb call). Linked phones: `null` until the helper sends it. |
| `nodes[].index` | int | n/a | Position in the same filtered list as `text`. Same index the model sees. Under `filter` it is NOT the position in the full dump. |
| `nodes[].class`, `label` | string | `""` | Same as `UiEl.class` and `UiEl.label`. |
| `nodes[].resource_id` | string | `""` | Same as `UiEl.id`. |
| `nodes[].bounds` | `[x1,y1,x2,y2]` ints | n/a | Screen pixels, same as `text`. |
| `nodes[].clickable/scrollable/checkable/editable/password/focused` | bool | `false` | `password` is true for password input types. |

Rules:
- `nodes` honours `filter` and `max_elements` the same way `text` does.
- Opt-in args (step 2, PR #2606): `with_activity: true` fills `activity` (one extra `dumpsys activity activities` call, shared with `adbForeground`); otherwise `activity` is `null`. `with_version: true` fills `app_version`. The Rust map code (steps 4-6) MUST pass `with_activity: true`, or fingerprints fall back to package-only.
- adb branch: change `uiDump` (:1293), `parseUiDump` (:830), `formatUiElement` (:851) and `readUi` (:1300). `app` comes from the `package` attribute of the root node. `activity` comes from the focused window (to confirm in step 2: `dumpsys window` vs `dumpsys activity top`; if it costs more than one extra adb call, leave `null`).
- Phone-Linked branch: `read_ui` for linked serials goes through `linkedCall` (:1197) to `phone-link-backend.ts` `readUi` (~:633), which returns only `{ok, text}`. Step 2 (PR #2606) adds a second structured `page_source` call and maps its nodes into the same fields. Step 3 makes `phone-link-backend.ts` return `nodes[]`, `app` and `activity` in the one `read_ui` call and drops the second call.
- Helper `page_source` (Kotlin, `UiTree.kt`, `PhoneLinkAccessibilityService.kt`) adds `activity` (class of the last `TYPE_WINDOW_STATE_CHANGED` event; to confirm in step 3) and per-node `checkable`. `clickable`, `scrollable` and `package` already exist (`UiTree.kt` :45-55).
- iPhone (OCR path): `app` = `""` unless the route reports one, `activity` = `null`, `nodes[]` from OCR boxes with `class: "Text"`, `resource_id: ""`, all flags false.
- Old clients: a client that does not send these fields means maps are off for that phone. The API MUST NOT error. A new Desktop with an old API ignores the extra fields.

Rust side: `phone_replay::parse_ui` and `UiEl` {class, label, id, rect} stay. Add a `UiNode` parse (in `phone_map.rs`) that reads `nodes[]` when present. If `nodes` is absent, `observe` and `route` are no-ops.

## 2. Screen fingerprint

Version field: `fp_version` (int, starts at `1`). It is stored in every map file at top level. A map file with a different `fp_version` than the running code is read as empty for matching and kept on disk until rewritten.

Inputs (Android, `fp_version: 1`):
1. `app` (package).
2. `activity` (or `""`).
3. The set of stable node keys outside the largest scrollable container.

Algorithm:
1. Find the scrollable node with the largest bounds area. Drop it and every node whose bounds lie inside it.
2. For each remaining node, key = `resource_id` if non-empty, else `class` (class-only keys are kept as a count-less set).
3. Drop keys whose `resource_id` looks dynamic (ends in digits only, or contains a hex run of 8+ chars). Drop all nodes with `password: true` from the key set.
4. Sort and de-duplicate the keys.
5. `fingerprint = "v1:" + hex(sha256(app + "\n" + activity + "\n" + keys.join("\n")))[0..16]`.

Matching a live screen against known screens:
1. Exact: same `fingerprint`.
2. Else, among screens with the same `app` and `activity`, the best Jaccard overlap of key sets, accepted at 0.8 or more. Key sets are stored as `keys` in the screen record for this (see 3).
3. Else the screen is new.

Weak screens (Compose or single-activity apps, no resource ids): when fewer than 3 keys have a `resource_id`, add the first non-empty top title label to the hash input and set `confidence: "low"`. Low-confidence screens match exact only.

iPhone (`fp_version: 1`, platform `ios`): input is OCR text from the top 12% and bottom 12% of the frame (lowercased, digits removed, words under 3 chars dropped), sorted and hashed the same way with `app` = `""` unless known. `confidence: "low"`. Exact match only. iOS maps are passive only (no Explore).

Scroll position MUST NOT change the fingerprint (fixture test in step 4: two scroll positions, one id).

## 3. Map file

Path: `data_dir/phone-maps/<platform>/<package>.json`. `platform` is `android` or `ios`. `<package>` is the package or bundle id, with any character outside `[A-Za-z0-9._-]` replaced by `_`. One writer per file; write by temp file then rename (same pattern as `phone-skills/`; to confirm in step 4). Maps are local, private, never synced. Typed text is never stored. Screen labels are stored (privacy: Forget deletes the file).

```json
{
  "version": 1,
  "fp_version": 1,
  "platform": "android",
  "app": "com.android.settings",
  "app_version": "14",
  "truncated": null,
  "screens": {
    "s1": {
      "id": "s1", "fingerprint": "v1:9f2c4a7e1b3d5a60", "keys": ["android:id/title", "android.widget.Switch"],
      "name": "Wi-Fi", "named_by": "brain",
      "activity": "com.android.settings.Settings$WifiSettingsActivity",
      "confidence": "high",
      "first_seen": "2026-11-18T10:00:00Z", "last_seen": "2026-11-18T10:05:00Z"
    }
  },
  "edges": [
    {
      "from": "s0", "to": "s1", "action": "tap",
      "locator": { "class": "android.widget.TextView", "label": "Network & internet", "resource_id": "android:id/title", "rel": [0.1, 0.2, 0.8, 0.07] },
      "ok": 4, "fail": 0, "consecutive_fail": 0,
      "app_version": "14", "last_ok": "2026-11-18T10:05:00Z", "last_fail": null,
      "routable": true
    }
  ]
}
```

| Field | Type | Notes |
|---|---|---|
| `version` | int | File schema version. `1`. |
| `fp_version` | int | Section 2. |
| `platform` | `"android"` or `"ios"` | |
| `app` | string | Package or bundle id. |
| `app_version` | string | Version name when known, else `""`. |
| `truncated` | null or `{reason, at}` | `reason`: `time_budget`, `screen_cap`, `stopped`, `step_watchdog`, `foreign_app`. Set by Explore only. Cleared by the next complete Explore. |
| `screens` | object id to Screen | Ids are `s<n>`, never reused. Cap 300 screens per app; past that, the least recently seen screen with no edges is dropped first, then the oldest. |
| `screens[].keys` | string[] | Fingerprint key set (section 2). Sorted. |
| `screens[].name` | string | One short line. With no brain: `activity short name + " " + top title`. |
| `screens[].named_by` | `"brain"`, `"auto"`, `"user"` | |
| `screens[].confidence` | `"high"` or `"low"` | |
| `edges[].from`, `to` | screen id | `from == to` edges are not stored. |
| `edges[].action` | `"tap"`, `"back"`, `"open_app"` | No `type`, `swipe`, `key`. |
| `edges[].locator` | `phone_replay::Locator` or null | `Locator` = `{class, label?, resource_id?, rel?: [x,y,w,h] (0..1), crop?}` (`phone_replay.rs` :283). Required for `tap`; null for `back` and `open_app`. `crop` is never stored in maps. |
| `edges[].ok`, `fail`, `consecutive_fail` | int | Counters. |
| `edges[].app_version` | string | Version when last verified. |
| `edges[].last_ok`, `last_fail` | RFC 3339 or null | |
| `edges[].routable` | bool | Default `true`. `false` for sensitive edges (never routed) and stale edges (section 10). |

An edge is unique by `(from, to, action, locator.resource_id or locator.label)`. Seeing it again bumps `ok` and `last_ok`.

Sensitive edges: when the guard says the tap is sensitive (`label_effect`, `is_secret_field`, `rule_effect`, or the model flagged `sensitive`), the edge is stored (so the screen graph stays connected) with `routable: false` and `sensitive: true`. `go_to` never uses it. The field `sensitive` (bool, default `false`) is part of the edge record.

Money apps (`phone_guard::is_money_app(app)`): no map file is written and none is read. `GET` shows nothing for them.

## 4. Routes in allternit-api

All are owner-scoped like `/phones/skills`. Register in `phone_map.rs` (route table in `lib.rs` and `main.rs`; to confirm in step 4 which of the two registers the `/phones/*` routers). Route overlap with `/phones/:serial/...` is checked by the smoke-boot (precedent: `/phones/skills/:name`).

| Route | Body or query | Result |
|---|---|---|
| `GET /phones/maps` | `?platform=android\|ios` optional | `{ ok: true, maps: [Summary] }` |
| `GET /phones/maps/:app` | `?platform=` optional (default: whichever exists, android first) | `{ ok: true, map: <map file> }` or 404 `map_not_found` |
| `DELETE /phones/maps/:app` | `?platform=` optional | `{ ok: true, deleted: bool }`. Deletes the file. Idempotent. |
| `POST /phones/maps/:app/explore` | `{ serial: string (required), minutes?: int (default 10, 1..30), max_screens?: int (default 60, 1..300) }` | 202 `{ ok: true, status: "running", run_id }` |

Summary:
```json
{ "app": "com.android.settings", "platform": "android", "app_version": "14", "name": "Settings",
  "screens": 12, "edges": 18, "routable_edges": 15, "stale_edges": 2,
  "truncated": null, "last_seen": "2026-11-18T10:05:00Z" }
```
`name` is the app label when known, else the package.

Stop: the existing `POST /api/aci/stop/:id` (`aci_routes.rs` :33, handler :1140). Today only `run_web_task_loop` checks `cancelled()` (`phone_task.rs` :760-770). Step 6 adds a cancel flag that `run_task_loop` and Explore both check before each step, fed by the same stop route. This also makes a phone task stoppable. The flag is keyed by `run_id`. Stop on an unknown or finished id keeps today's behaviour.

Errors (all `{ ok: false, error: <code>, message }`):

| HTTP | `error` | When |
|---|---|---|
| 400 | `invalid_args` | Bad body, minutes or max_screens out of range, bad `:app`. |
| 400 | `explore_android_only` | `serial` is an iPhone. |
| 403 | `money_app` | `is_money_app(:app)`. |
| 404 | `map_not_found` | No file. |
| 404 | `phone_not_found` | Unknown serial. |
| 409 | `explore_running` | An Explore run is already active for that serial. |
| 409 | `phone_busy` | A task or skill run is active on that serial (to confirm in step 6 how busy is detected). |
| 502 | `phone_unreachable` | The phone cannot be read. |

Run kind: Explore registers a run with kind `explore` in `phone_runs` (existing kinds are `phone_task` for tasks and `skill` for skill runs; `post_run_card` kinds are `phone_task` and `computer_task`; to confirm in step 6 where run kinds are enumerated). Events: it emits `phone.run.completed` with `source: "explore"` and a report (section 7.5).

## 5. Task model: `go_to` and `look`

`phone_task.rs` `schema()` (:70-80) action enum gains two values. The new enum is `["tap","type","swipe","back","home","key","open_app","go_to","look","done","fail"]`. The schema gains one property: `"screen": { "type": "string", "description": "Known screen id or name from the map list." }`. `additionalProperties: false` stays.

| Action | Fields | Meaning |
|---|---|---|
| `go_to` | `screen` (id or exact name), `reason` | Replay the shortest routable path from the current screen to `screen`. No model call during replay. |
| `look` | `reason` | Send one screenshot with the next model call. Costs no phone action. |

Prompt section (added to `prompt()`): when the foreground app has a map and the current screen is matched, list up to 20 known screens:

```
Known screens in <app> (use go_to with the id):
- s1 "Wi-Fi" (here)
- s2 "Bluetooth"
```
Order: current screen first, then screens reachable by routable edges by fewest hops, then most recently seen. Names only, no labels from other screens. If the map is missing, not matched, off (old client) or the app is a money app, the section is omitted and `go_to` is rejected with `invalid action: no map`.

Replay algorithm for `go_to(screen)`:
1. Read the UI (`read_ui` with nodes). Match the current screen (section 2). If no match: fail the action (`not_on_known_screen`).
2. Breadth-first search over `routable` edges for the shortest path. No path: fail (`no_route`).
3. For each edge, in order:
   a. Resolve the locator against the live nodes with the same resolver `phone_replay` uses for skills (`resolve`). No match: edge fail.
   b. `phone_guard::gate` with the task's `TaskCtx` (the task's own `source`). Every replayed edge goes through the gate, no exceptions. A hold or denial ends the replay and is handled exactly as a normal step (hold: the run waits as today; denied: the step fails).
   c. `act` (tap, back, or open_app).
   d. `read_ui`. Check `app` equals the edge's package. Check the new screen matches the edge's `to` (exact or overlap rule).
4. On mismatch at 3a or 3d: mark that edge failed (section 10), stop replaying, add a history line, return to the normal loop with the next model call seeing the real screen. Never retry the edge in the same run.
5. Edge ok: bump `ok`, `consecutive_fail = 0`, `last_ok`, `app_version`. Count it in `map_steps`.
6. Replay stops after at most 12 edges. Reaching `screen` ends the action as ok.

Foreground check fix: after `phone.open_app` succeeds, `app` MUST be taken from `read_ui.app` (not the requested name, today at `phone_task.rs` :584-595). If `read_ui.app` is empty, keep today's behaviour. A foreground that differs from the request is recorded in history ("opened X, foreground is Y") and the guard classifies against the real foreground.

Task input gains `vision` on `TaskIn`: `"auto"` (default) or `"always"`.
- `auto`: send the tree only when it is rich; send a screenshot when the tree is poor (fewer than 5 nodes with a label or resource id), after a failed step, after a `look` action, or when a hold needs its picture.
- `always`: send a screenshot with every model call (today's behaviour of vision-first runs).
- The benchmark (step 13) decides whether `auto` stays the default. Unknown value: 400 `invalid_args`.
- Exposed in `phone.task` and `phone_task` MCP args only if step 5 finds a place for it; to confirm in step 5.

Passive observation: every task and skill run calls `observe(prev_screen, action, locator, new_screen)` after each acting step that changed screen. Observation skips money apps, foreign packages (the new `app` differs from the map's app), password screens (any `password: true` node), and typed steps. A step that changed the screen after `type` is not recorded.

## 6. Report fields added

The task and skill report (`run_task_loop` return in `phone_task.rs` :611-624, and the skill report in `phone_replay.rs`) adds:

| Field | Type | Default | Meaning |
|---|---|---|---|
| `tokens_in` | int | 0 | Sum of input tokens from model replies. Today `run_task_loop` drops usage (`reply.map(\|(t, _)\| ...)` at :485); keep it. 0 when the brain reports none. |
| `tokens_out` | int | 0 | Same, output tokens. |
| `map_steps` | int | 0 | Edges replayed with no model call. |
| `screenshots_sent` | int | 0 | Screenshots attached to model calls. |
| `fallbacks` | int | 0 | `go_to` replays that ended on a mismatch or no route, so the loop fell back. |

`model_calls` stays. `phone_runs::run_event` copies the five fields into `phone.run.completed`. Old clients ignore them.

Explore report (section 7.5) uses `ok`, `status`, `run_id`, `serial`, `duration_ms` and adds its own fields.

## 7. Guard source `explore` and the Explore run

### 7.1 Guard

`phone_guard::SOURCES` (:76, now 8 entries) gains `"explore"`. `clean_source` keeps unknown values as `api`.

Rule: for source `explore`, any decision that would be a hold (`Gate::Hold`) is a deny. Explore never waits for a person. Deny ends that candidate only; Explore records nothing for it and continues. To confirm in step 4: where `phone_guard::check` turns a rule into hold vs deny, and whether a new `Decision::Deny` reason code is needed (proposed: reason `explore_denies`).

### 7.2 Candidate filter

Explore reads `read_ui` and picks candidates from `nodes[]`. A node is NOT a candidate when any of these is true:
- `phone_guard::label_effect(el)` returns something.
- `phone_guard::is_secret_field(el)` is true, or `password` is true.
- `editable` or `checkable` is true.
- `clickable` is false.
- Its bounds lie inside the status bar or navigation bar (top 4% or bottom 6% of the screen height).
- It is not the first item of its scroll container in the list order. That is: one candidate per scrollable container (the first clickable child). A container is a scrollable node; the rule applies to every node inside it.
- The app is a money app: Explore refuses the whole app with 403 `money_app` before any tap.

Every chosen tap still goes through `phone_guard::gate` with source `explore`.

### 7.3 Loop

1. Open the app (`open_app` by package). Read the UI. Match or create the root screen.
2. Depth-first, with a visited set of screen ids. For each candidate on the current screen not yet tried: tap, read, then record `from -> to` (observe) when the new screen is in the same package. If the screen is new, descend; else go back.
3. Backtrack with `back` (one press) and check the fingerprint returned to the parent. If not, replay from the root with known edges.
4. Foreground change (the new `app` differs from the map's package): press `back` twice, check again; still foreign: relaunch the app by package. Nothing from the foreign screen is recorded, not the screen and not the edge. Count a `foreign_app` event; the third one in a run ends Explore with `truncated.reason = "foreign_app"`.
5. Step watchdog: each act + read must finish within 15 s. On timeout the step fails, Explore presses `back`, and goes on. Two timeouts in a row end Explore with `step_watchdog`.
6. Save the map file after every new screen (not only at the end).
7. Budget: `minutes` wall-clock (default 10, max 30) and `max_screens` (default 60, max 300). Whichever comes first ends the run. The cancel flag is checked before every step.
8. `truncated` is set on the map when the run ended for any reason other than finishing every candidate: `{ "reason": "time_budget" | "screen_cap" | "stopped" | "step_watchdog" | "foreign_app", "at": "<RFC 3339>" }`. A run that visited everything sets `truncated: null`.
9. Naming: new screens are named in one batched brain call at the end (and on stop), on the user's own brain. With no brain or a failed call: `named_by: "auto"`.

### 7.4 Re-check

"Re-check" is an Explore run limited to known edges: it replays each edge from its `from` screen and updates `ok` / `fail` (section 10). It uses the same route with body field `recheck: true` (bool, default `false`; to confirm in step 6 whether this is worth a separate flag or the default behaviour of a second Explore).

### 7.5 Explore report

```json
{
  "ok": true, "status": "done", "kind": "explore", "run_id": "...", "serial": "...", "app": "com.android.settings",
  "screens_total": 12, "screens_new": 9, "edges_total": 18, "edges_new": 14,
  "taps": 31, "skipped": { "sensitive": 3, "editable": 4, "checkable": 6, "dynamic": 2 },
  "truncated": null, "duration_ms": 412000, "model_calls": 1, "tokens_in": 0, "tokens_out": 0
}
```
`status` is `done`, `stopped`, or `failed`. A truncated run is `status: "done"` with `truncated` set, except `stopped`.

## 8. Cloud relay and MCP

Files: `cmd/allternit-cloud-api/src/routes/platform_v1/phones.rs`. No DB migration.

- `GET /v1/phones/maps` relays to the owner's runtime `GET /phones/maps` (the same relay used for `/v1/phones/runs`) and returns `{ ok: true, maps: [Summary] }`. Optional `?serial=` picks the runtime (to confirm in step 7: how `phone_runtime` / `latest_phone_runtime` are chosen). Runtime offline: the same error the other `/v1/phones/*` routes return (to confirm in step 7).
- MCP tool `phone_maps` (added to `mcp_tools()`, ~:1158), read only, input `{ "serial": string?, "app": string? }`, no `additionalProperties`. Without `app`: the Summary list. With `app`: one summary plus the screen names and the edge count (not locators and not labels).
- `phone_task` MCP schema: gains `"vision": { "enum": ["auto","always"] }` only if step 5 exposes it.
- Explore and Forget are NOT exposed over MCP and are not relayed in the cloud API. They stay owner-only on the Desktop, web, app or Gizzi.
- `is_mcp_tool` picks up `phone_maps` automatically (it checks the `phone_` prefix against `mcp_tools()`).

## 9. Desktop tools and Gizzi commands

Registry: `surfaces/allternit-desktop/src/main/phone-tools.ts`.

`phone.maps`: no side effects, never guarded.
```json
{ "type":"object","properties":{ "serial":{"type":"string","maxLength":128}, "app":{"type":"string","maxLength":200} },"additionalProperties":false }
```
Result: `{ ok:true, maps:[Summary] }` without `app`; `{ ok:true, map:<map file> }` with `app`.

`phone.explore_app`: starts Explore. Android only.
```json
{ "type":"object","required":["app"],"properties":{ "serial":{"type":"string","maxLength":128}, "app":{"type":"string","maxLength":200}, "minutes":{"type":"integer","minimum":1,"maximum":30,"default":10}, "max_screens":{"type":"integer","minimum":1,"maximum":300,"default":60} },"additionalProperties":false }
```
Result: `{ ok:true, run_id, status:"running" }` or the section 4 error. Both tools call the API routes in section 4. Stopping an Explore uses the existing stop path (`POST /api/aci/stop/:id`); `phone-runs.ts` (step 8) adds the client call. Forget uses `DELETE /phones/maps/:app`.

Gizzi Code (`P/cmd/gizzi-code/src/cli/commands/phone.ts`, today `list|task|runs`):

| Command | Does |
|---|---|
| `gizzi phone maps [--app <pkg>] [--json]` | Lists summaries, or one map's screens and routes. |
| `gizzi phone explore <app> [--serial <s>] [--minutes <n>] [--max-screens <n>] [--stop <run_id>]` | Starts Explore and prints `run_id`; `--stop` stops one. Prints `truncated` with its reason when the run ends. |
| `gizzi phone forget-map <app> [--yes]` | Deletes the map. Asks first without `--yes`. |

The task outcome line adds `map_steps`, `tokens_in + tokens_out`, `model_calls` (to confirm in step 11 how the outcome line is built today).

## 10. Staleness

- A failed replay of an edge (section 5 step 4): `fail += 1`, `consecutive_fail += 1`, `last_fail = now`.
- At `consecutive_fail >= 2` the edge gets `routable: false` (when not sensitive; sensitive edges are always `false`). Stale edges stay in the file.
- A later passive observation or Re-check that sees the same edge work sets `consecutive_fail = 0`, `ok += 1`, `last_ok = now`, and `routable: true` again (never for a sensitive edge).
- A screen in `read_ui` that matches no known screen of a mapped app adds a new screen (confidence from section 2). It does not touch existing screens.
- `app_version` change (the live version differs from `map.app_version`): the map's `app_version` is updated, each edge keeps its own `app_version`, and the summary reports `stale_edges` = routable edges whose `app_version` differs from the live one. Those edges are still routable; a failure there counts as usual. How the live version is read: to confirm in step 2 (adb `dumpsys package <pkg> | grep versionName`, or the helper).
- No schedule. No cron. Maps are never re-crawled unless the owner asks.

## 11. Surfaces

| Item | Plan step | Where |
|---|---|---|
| 1 `read_ui` nodes, adb + linked | 2 | `phone-tools.ts` |
| 1 helper `activity`, `checkable` | 3 | `UiTree.kt`, `PhoneLinkAccessibilityService.kt` |
| 2, 3 fingerprint, map file, store, observe, route, edge stats, `GET/DELETE /phones/maps` | 4 | `phone_map.rs`, `lib.rs`, `main.rs` |
| 7.1 guard source `explore` | 4 | `phone_guard.rs` |
| 5 `go_to`, `look`, prompt section, vision, foreground fix | 5 | `phone_task.rs`, `phone_replay.rs` |
| 6 report fields | 5 | `phone_task.rs`, `phone_runs.rs` |
| 4, 7.2-7.5 Explore, stop flag, run kind `explore`, `POST .../explore` | 6 | `phone_map_explore.rs`, `phone_task.rs`, `aci_routes.rs` |
| 8 cloud relay, MCP `phone_maps` | 7 | `routes/platform_v1/phones.rs` |
| 9 Desktop tools `phone.maps`, `phone.explore_app` | 8 | `phone-tools.ts`, `phone-runs.ts` |
| Web and PWA "App maps" tab | 9a, 9b | allternit-ai `PhoneMapsTab.tsx`, `PhoneRunsPanel.tsx` |
| iOS list, Explore, Stop, Forget | 10 | `AppPhonesSettingsView.swift`, `PhoneTaskClient.swift` |
| 9 Gizzi commands | 11 | `gizzi-code/.../phone.ts` |
| Docs guide, nav, API reference | 12 | `surfaces/docs` |
| Benchmark | 13 | `scripts/phone-map-bench.mjs` |
| Crate split entry for `phone_map`, `phone_map_explore` in unit `phone` | 4 | `handoffs/api-crate-split-2026-11-17/layers-draft.json` (not in the repo; coordinate with the train session) |

## 12. To confirm

| Id | Item | Step |
|---|---|---|
| C1 | adb `activity` source and cost | 2 |
| C2 | Helper `activity` from window-state events | 3 |
| C3 | Atomic write pattern in `phone-skills/` and which of `lib.rs` / `main.rs` registers phone routers | 4 |
| C4 | Where hold becomes deny in `phone_guard::check`; reason code for `explore` | 4 |
| C5 | Whether `vision` is exposed in `phone.task` and the MCP `phone_task` | 5 |
| C6 | Run-kind enumeration in `phone_runs` and the busy check for `409 phone_busy` | 6 |
| C7 | `recheck` flag vs default second-Explore behaviour | 6 |
| C8 | Cloud relay runtime selection and offline error | 7 |
| C9 | Outcome line in Gizzi Code | 11 |
| C10 | How the live app version is read | 2 |
| C11 | Resolver name and signature shared with skills (`resolve`) and where the 12-edge replay cap sits | 5 |
