# Contract: Phone Use (iPhone without iCTRL), 2026-11-16

Status: draft, from `/Users/joe/.claude/plans/iphone-phone-use.md` (approved by Eoj 2026-11-16, with Amendments). Every lane (Desktop, ai/PWA, iOS, Android, Gizzi Code, runtime, docs) builds against this file. Anything here marked UNVERIFIED stays so until the plan step 1 results (P1 to P6) are in. Real-device behaviour is "not checked" until run.

Key words: MUST, MUST NOT, MAY are normative.

## 1. Routes and ids

Route ids are lowercase strings. They appear in `route` in every result (section 4) and in health reports (section 5).

| id | What | Eyes | Taps |
|---|---|---|---|
| `official` | Apple-supported actions: Shortcuts (x-callback, or an App Intent that returns a result), HomeKit, EventKit, Contacts, other Apple frameworks called by the Allternit app | result payload | none |
| `mirror` | macOS iPhone Mirroring driven by Desktop (`cmd/allternit-mirror`) | Mirror window frame + OCR | Mac pointer and keys into the window |
| `bt-mac` | Mac acting as Bluetooth keyboard and mouse for the phone | broadcast frames read by Desktop | HID from Mac |
| `bt-android` | Allternit Android app acting as Bluetooth keyboard and mouse (`BluetoothHidDevice`) | broadcast; on-phone read or the bot's vision | HID from Android |
| `bt-apple` | A second iPhone or iPad running the App Store Allternit app as Bluetooth keyboard and mouse. A MAIN route (the common setup is two Apple devices and no Mac). Needs P5 to pass; described to App Review as a Bluetooth keyboard/mouse remote. The app declares the `bluetooth-peripheral` background mode, but the person keeps Allternit open on the second device: the app keeps its screen awake while keyboard mode runs, and if iOS suspends it the link drops and every key and button is released (section 9.2) | broadcast | HID from the second device |
| `usb-agent` | USB Agent board (cable, or Bluetooth mode) | AirPlay or broadcast | board HID |
| `legacy-helper` | Existing iCTRL helper. DEPRECATED: no new setup; removed 30 days after plan step 16 passes, with an in-app notice | helper | helper |

### 1.1 Router order

1. If an `official` action covers the request (a matching `phone.actions` entry, a named shortcut, a Home or calendar/reminder/contact call), use `official`. Always first.
2. Otherwise a tap route is chosen by lock state and health. First healthy row wins:

| Order | Route | Used when | Target act + verify (target, not measured) |
|---|---|---|---|
| 2a | `mirror` | phone locked, Mac unlocked, same Apple Account, not EU | 1.5 s |
| 2b | `bt-mac` | phone unlocked or in use, a Mac is linked | 3 s |
| 2c | `bt-apple` | second iPhone/iPad paired (MAIN route per D1; needs P5 to pass) | 4 s |
| 2d | `bt-android` | no Mac or second Apple device, or those unhealthy | 4 s |
| 2e | `usb-agent` | any Desktop OS, board attached | 3 s |
| last | `legacy-helper` | already-linked users only, until removal | unchanged |

3. If the Allternit app is not foreground and a tap route is healthy, the router opens it through Spotlight (`phone.open_app`) and then runs the shortcut.
4. If no route is healthy, the result is `ok:false` with exactly one `needs_you` state (section 5). The router MUST NOT try a second route silently after a `needs_you` that needs the user, and MUST NOT retry a guard-denied action on another route.
5. The caller MAY pin a route with `route` (section 3.1). A pinned route that is unhealthy returns its `needs_you`; it does not fall back.

Windows and Linux Desktop have no native Bluetooth sender: they use `bt-android`, `bt-apple` or `usb-agent`.

## 2. Common types

```json
{
  "$defs": {
    "RouteId": { "enum": ["official","mirror","bt-mac","bt-android","bt-apple","usb-agent","legacy-helper"] },
    "NeedsYouCode": { "enum": ["MAC_LOCKED","MIRROR_WINDOW_HIDDEN","MIRROR_PHONE_IN_USE","BROADCAST_STOPPED","BT_NOT_PAIRED","ASSISTIVETOUCH_OFF","FULL_KEYBOARD_ACCESS_OFF","APP_NOT_FOREGROUND","SHORTCUT_NOT_INSTALLED","HELPER_RETIRING","MAC_PERMISSION_MISSING","DEVICE_ACCESS_OFF"] },
    "Serial": { "type": "string", "maxLength": 128, "description": "Phone id. App-linked iPhones are \"app:<id>\"." },
    "Element": {
      "type": "object",
      "description": "Today's element shape from phone.read_ui. OCR results fill the same fields.",
      "properties": {
        "class": { "type": "string", "description": "\"Text\" for OCR results" },
        "label": { "type": "string" },
        "id": { "type": "string", "description": "empty for OCR results" },
        "bounds": { "type": "array", "items": { "type": "integer" }, "minItems": 4, "maxItems": 4, "description": "[x1,y1,x2,y2] in screenshot pixels" }
      },
      "required": ["class","label","bounds"]
    }
  }
}
```

## 3. Tool schemas

All tools take an optional `serial` (Serial), and an optional `route` (RouteId, pins the route; omit for automatic). `additionalProperties: false` everywhere. Tool names are defined in `surfaces/allternit-desktop/src/main/phone-tools.ts` (the single registry); the runtime guard table is `cmd/allternit-api/src/phone_guard.rs`.

### 3.1 New tools

`phone.describe`: what this phone can do right now. No side effects, never guarded.

```json
{ "type":"object","properties":{"serial":{"$ref":"#/$defs/Serial"}},"additionalProperties":false }
```
Result `detail`:
```json
{ "type":"object","required":["routes","foreground_app","locked"],
  "properties":{
    "routes":{"type":"array","items":{"type":"object","required":["route","healthy"],
      "properties":{"route":{"$ref":"#/$defs/RouteId"},"healthy":{"type":"boolean"},"needs_you":{"$ref":"#/$defs/NeedsYouCode"},"message":{"type":"string","description":"the filled section 5 sentence, present with needs_you"},"covers":{"type":"array","items":{"enum":["shortcuts","actions","home","tap","type","swipe","key","screenshot","read_ui"]}}}}},
    "locked":{"type":["boolean","null"],"description":"null when unknown"},
    "foreground_app":{"type":["string","null"]},
    "shortcuts":{"type":"array","items":{"type":"string"},"description":"installed Allternit shortcut names"},
    "home":{"type":"object","properties":{"available":{"type":"boolean"}}}
  } }
```

`phone.shortcuts.run`: run any shortcut the user has.
```json
{ "type":"object","required":["name"],"properties":{
  "serial":{"$ref":"#/$defs/Serial"},
  "name":{"type":"string","maxLength":200},
  "input":{"type":["string","object","array","number","boolean","null"],"description":"Passed as the shortcut input; objects are sent as JSON text"},
  "timeout_s":{"type":"integer","minimum":1,"maximum":120,"default":30}},
  "additionalProperties":false }
```
Result `detail`: `{ "output": <string|object|null>, "returned_via": "app_intent"|"x_callback" }`. Missing shortcut: `needs_you: SHORTCUT_NOT_INSTALLED`. Guarded (section 8).

`phone.actions.list`: the action catalog (section 7).
```json
{ "type":"object","properties":{"serial":{"$ref":"#/$defs/Serial"},"app":{"type":"string","maxLength":64,"description":"catalog app id, e.g. \"slack\"; omit for all"},"kind":{"enum":["create","send","open","search","read","control","other"]},"installed_only":{"type":"boolean","default":false}},"additionalProperties":false }
```
Result `detail`: `{ "apps": [ { "id","name","category","actions":[ActionEntry] } ] }`, ActionEntry from section 7.2. Every app lists its actions (from `device.describe`'s `action_list`, section 9.1); an app version from before `action_list` lists `action_count` and no actions.

`phone.actions.run`: run one catalog action.
```json
{ "type":"object","required":["app","action"],"properties":{
  "serial":{"$ref":"#/$defs/Serial"},
  "app":{"type":"string","maxLength":64,"description":"catalog app id, or \"allternit\" for the built-in Actions shortcut and framework actions"},
  "action":{"type":"string","maxLength":128,"description":"ActionEntry.id"},
  "params":{"type":"object","additionalProperties":true},
  "timeout_s":{"type":"integer","minimum":1,"maximum":120,"default":30}},
  "additionalProperties":false }
```
Result `detail`: `{ "output": object|null, "dispatched_as": "framework"|"actions_shortcut"|"app_shortcut"|"app_intent" }`. Unknown app or action: `ok:false, detail.error:"unknown_action"`. Params failing the action's `params` list: `detail.error:"bad_params"` with `detail.missing: string[]`. The built-in "Allternit Actions" shortcut dispatches on a JSON `action` field. Desktop checks `app` and `action` against `device.describe`'s action lists, then sends `device.shortcuts.run` with the app's shortcut name and the input `{ "action", "params" }` (there is no separate `device.actions.run` message).

`phone.home.list`: `{ serial?, kind?: "light"|"switch"|"lock"|"thermostat"|"garage"|"sensor"|"scene" }`. `detail`: `{ "devices":[{ "id","name","room","kind","state":object }], "scenes":[{ "id","name" }] }`. Read only, never guarded.

`phone.home.set`: `{ serial?, id: string, state: object }`, e.g. `{ "on": true, "brightness": 40 }`, `{ "locked": false }`. `detail`: `{ "id","state":object }` (state read back). Guarded when `kind` is lock, garage or security (section 8).

`phone.home.scene`: `{ serial?, id: string }`. `detail`: `{ "id","ran":true }`. Guarded when the scene contains a lock, garage or security accessory.

`phone.home.*` is `official` only (HomeKit in the iOS app). No tap fallback.

### 3.2 Existing tools on every route

Schemas stay as in `phone-tools.ts` (`phone.screenshot`, `phone.tap{x,y}`, `phone.type{text<=500}`, `phone.read_ui{filter?{label,type}}`, `phone.find_and_tap{element_id,match?}`, `phone.swipe{x1,y1,x2,y2 | direction}`, `phone.home`, `phone.back`, `phone.key{key|combo}`, `phone.open_app{app}`) with `serial` and `route` added. Behaviour per tap route:

| Tool | `mirror` | `bt-mac`, `bt-android`, `bt-apple` | `usb-agent` |
|---|---|---|---|
| `phone.screenshot` | frame of the Mirror window | latest broadcast frame | AirPlay or broadcast frame |
| `phone.read_ui` | OCR of the frame | OCR of the frame (Desktop reads; `bt-android` MAY read on phone) | OCR of the frame |
| `phone.tap` | click in window | pointer move + click | `UsbPointer.tap` |
| `phone.type` | keys into window | HID keys (ASCII only; non-ASCII UNVERIFIED, returns `bad_params` for characters the layout cannot send) | `UsbPointer.type` |
| `phone.swipe` | drag in window | pointer drag | `UsbPointer.drag` |
| `phone.find_and_tap` | `read_ui` then tap centre of the best label match | same | same |
| `phone.home` | Mirror Home | Cmd+H | `UsbPointer.goHome` |
| `phone.back` | Mirror back gesture | left-edge swipe | `UsbPointer.goBack` |
| `phone.key` | combos into window | HID combo (Cmd+Space etc.) | `UsbPointer.key` |
| `phone.open_app` | Spotlight by name | Cmd+Space, type, enter | same |

Rules:
- `phone.read_ui` MUST return `detail.elements: Element[]` (OCR text lines as `class:"Text"`, `id:""`, bounds in screenshot pixels) plus `detail.text` (lines joined by `\n`). Same shape as the helper's today, so existing readers keep working. `filter.label` is a case-insensitive substring match.
- Tap coordinates are pixels of the latest `phone.screenshot` image. Each result carries `coordinate_space` (`"pixels"`).
- On `official` these tools return `ok:false, detail.error:"not_supported"`.
- `legacy-helper` behaviour is unchanged.

## 4. Result envelope and act-then-verify

Every `phone.*` tool result:
```json
{ "type":"object","required":["ok","route","verified","detail"],"properties":{
  "ok":{"type":"boolean"},
  "route":{"$ref":"#/$defs/RouteId"},
  "verified":{"type":"boolean","description":"true only when a post-action check confirmed the effect; read-only tools set true when the read succeeded"},
  "needs_you":{"type":"object","required":["code","message"],"properties":{"code":{"$ref":"#/$defs/NeedsYouCode"},"message":{"type":"string","description":"the user-facing sentence from section 5"}}},
  "detail":{"type":"object","additionalProperties":true} } }
```
- `needs_you` is present only when `ok` is false and the user must act. `ok:false` without `needs_you` is a failure (`detail.error` is a short code, `detail.message` a plain sentence).
- `ok:true, verified:false` means the action was sent but not confirmed. Callers MUST NOT claim success from it.
- Screenshot data stays in `detail` (`image`, `width`, `height`, `coordinate_space`) as today.

### 4.1 Act-then-verify (tap routes)

For `phone.tap`, `phone.type`, `phone.swipe`, `phone.key`, `phone.home`, `phone.back`, `phone.open_app`, `phone.find_and_tap`:

1. Capture frame A (before).
2. Send the action.
3. Wait for the screen to settle (quiet window; targets in section 1.1 are upper bounds), capture frame B (after).
4. Changed-screen check: frames differ beyond a threshold (grayscale diff, as `UsbPointer` already does). For `phone.type`, the typed text MUST appear in OCR of frame B when the field is visible; otherwise only the changed-screen check applies.
5. If unchanged: retry the action up to 2 more times (3 sends in total), re-capturing after each. For `phone.tap` on a pointer route, a retry MUST first re-home the pointer (section 6).
6. Still unchanged: return `ok:false, verified:false, detail.error:"no_change"` with frames A and B hashes. No `needs_you` unless a health check fails (then that code).

Results include `detail.attempts` (1 to 3) and `detail.changed` (boolean). `verified` is true iff the check passed.

Official actions are verified by their result payload (`detail.output`), with a 30 s default timeout.

## 5. Health and needs-you states

Health is evaluated before each tap action and reported by `phone.describe`. Each code has one fixed sentence (shown to the user as-is):

| Code | Raised when | User-facing sentence |
|---|---|---|
| `MAC_LOCKED` | `mirror` needed and the Mac is locked | Unlock your Mac so I can use your iPhone through it. |
| `MIRROR_WINDOW_HIDDEN` | iPhone Mirroring window not visible or not found | Open iPhone Mirroring on your Mac and keep its window showing. |
| `MIRROR_PHONE_IN_USE` | Mirroring reports the phone is in use | Your iPhone is in use right now. Lock it, or I can use another way if you let me. |
| `BROADCAST_STOPPED` | no live frames from the Allternit broadcast | Start the screen broadcast in the Allternit app on your iPhone. |
| `BT_NOT_PAIRED` | chosen Bluetooth route has no paired phone | Pair your iPhone with this Bluetooth keyboard and mouse in Settings > Bluetooth. |
| `ASSISTIVETOUCH_OFF` | pointer route, pointer not seen or AssistiveTouch reported off | Turn on AssistiveTouch on your iPhone (Settings > Accessibility > Touch). |
| `FULL_KEYBOARD_ACCESS_OFF` | key navigation needed and Full Keyboard Access is off | Turn on Full Keyboard Access on your iPhone (Settings > Accessibility > Keyboards). |
| `APP_NOT_FOREGROUND` | a shortcut needs Allternit in front and no tap route can open it | Open the Allternit app on your iPhone. |
| `SHORTCUT_NOT_INSTALLED` | `official` shortcut name not found | Add the "<name>" shortcut in Allternit Settings > Device Access > iPhone actions. |
| `HELPER_RETIRING` | `legacy-helper` selected | The old Allternit helper is being retired. Switch to the app and set up a new route before <date>. |
| `MAC_PERMISSION_MISSING` | `mirror` or `bt-mac` needs a macOS permission Allternit does not have (Screen & System Audio Recording, Accessibility; Bluetooth for `bt-mac`) | Allow Allternit in System Settings > Privacy & Security > <name> on your Mac. |
| `DEVICE_ACCESS_OFF` | an `official` call needs a Device Access switch (Shortcuts, Home) that is off in the Allternit app | Turn on <name> in the Allternit app on your iPhone (Settings > Device Access). |

`<name>` and `<date>` are filled by the runtime (for `MAC_PERMISSION_MISSING` the pane name, for `DEVICE_ACCESS_OFF` the switch name). `MIRROR_PHONE_IN_USE`, `BT_NOT_PAIRED`, `ASSISTIVETOUCH_OFF`, `FULL_KEYBOARD_ACCESS_OFF`, `MAC_PERMISSION_MISSING` have no `official` equivalent. `DEVICE_ACCESS_OFF` ends the call (`detail.error:"device_access_off"`): no other route has that switch. A `needs_you` for a route the router could skip is only returned when no other healthy route exists (section 1.1 step 4).

Detection of `ASSISTIVETOUCH_OFF` and `FULL_KEYBOARD_ACCESS_OFF` is by observation (pointer absent in broadcast; keyboard navigation not responding), not by an API. UNVERIFIED (P3, P4).

## 6. HID sender interface

One keyboard/mouse core, four senders: Mac (`bt-mac`), Android (`bt-android`), second Apple device (`bt-apple`), USB Agent board (`usb-agent`). The method set is taken from `UsbPointer` in `surfaces/allternit-desktop/src/main/phone-usb-agent-pointer.ts`. Coordinates are normalized to the screenshot (0..1 on each axis) at the interface; the core converts. Senders implement only transport.

```ts
interface HidPointer {
  calibrate(): Promise<boolean>;                         // find pointer at CALIBRATION_STOPS (150, 350); false if not found
  point(x: number, y: number, aim?: boolean): Promise<void>;
  tap(x: number, y: number, aim?: boolean): Promise<void>;       // aim defaults true
  drag(points: Array<[number, number]>, hold?: boolean): Promise<void>; // hold defaults true
  scroll(x: number, y: number, lines: number): Promise<void>;
  type(text: string): Promise<void>;                     // ASCII
  key(combo: string): Promise<void>;                     // "enter", "cmd+space", ...
  goHome(): Promise<void>;
  goBack(): Promise<void>;
  reset(): Promise<void>;                                // drive pointer to the corner
  forget(): void;                                        // drop calibration
  where(): [number, number] | null;
  calibration(): Calibration | null;
}
```
Constants carried over from the existing core: re-home to the corner every `REHOME_EVERY = 10` gestures; `NUDGE = 12`; `AIM_TOLERANCE = 0.008`; `MAX_MISS = 0.06`. Pointer moves are relative (a Bluetooth mouse has no absolute position), so every sender MUST support the corner reset and the core MUST re-home on that count. A sender-level failure is `BoardError`-shaped: `{ ok:false, code, message }` and maps to a section 5 code where one applies.

Sender transport interface (what each sender implements):
```ts
interface HidTransport {
  status(): Promise<{ paired: boolean; connected: boolean; name?: string }>;
  pointerMove(dx: number, dy: number): Promise<void>;     // relative, integer units
  pointerClick(button: 'left'|'right', down?: boolean, up?: boolean): Promise<void>;
  pointerReset(): Promise<void>;                          // large negative moves to the corner
  key(usage: number, modifiers: number, down: boolean): Promise<void>; // USB HID usage page 0x07
  typeText(text: string): Promise<void>;
  scroll?(lines: number): Promise<void>;                  // wheel lines, positive = down
}
```
`scroll` is optional: the core (`phone-hid-core.ts`) reports scrolling as unsupported for a sender without it; `bt-android` has it (`hid.scroll`).

Mac: our own code in `cmd/allternit-bthid` (P2 decides if the Mac can act as a HID device; UNVERIFIED). Android: our own code, `BluetoothHidDevice`, under `phoneshare/.../core/`. Second Apple device: our own code in the iOS app (public Bluetooth API, P5). Board: existing firmware path.

## 7. Action catalog

### 7.1 Source

`phone-actions-catalog.json` (built 2026-11-16; kept with the step 2 handoff, shipped with the app). Shape:

```json
{ "built": "2026-11-16", "note": "string",
  "apps": [ {
    "id": "slack", "name": "Slack", "category": "string",
    "ios": {
      "appIntents": [ { "title": "string", "id": "string", "params": ["string"], "opensApp": true, "isDiscoverable": true } ],
      "sirikitIntents": [],
      "urlSchemes": ["string"],
      "linkTemplates": [],
      "webResearchIntents": [ { "action":"string","params":["string"],"kind":"create|send|open|search|read|control|other","source":"url","confidence":"string" } ]
    },
    "android": { "intents": [] },
    "source": { "fromApp": true, "packageErrors": [] } } ] }
```
101 apps; 459 `appIntents`, of which 147 have `isDiscoverable:false` (widget or configuration only) and 312 are usable.

### 7.2 Filter and ActionEntry

- An App Intent is usable only if `isDiscoverable !== false` (a missing field counts as usable). Widget and configuration intents (`isDiscoverable:false`) MUST NOT appear in `phone.actions.list` and MUST NOT be callable by `phone.actions.run`.
- `webResearchIntents` entries are community-sourced: listed with `confidence` and `source`, and runnable only through a shortcut the user added (never invented).
- The runtime builds `ActionEntry` from usable intents:

```json
{ "type":"object","required":["id","title","kind","params","dispatch","installed"],"properties":{
  "id":{"type":"string","description":"stable; the App Intent id, or the framework action id for app \"allternit\""},
  "title":{"type":"string"},
  "kind":{"enum":["create","send","open","search","read","control","other"]},
  "params":{"type":"array","items":{"type":"string"}},
  "opens_app":{"type":"boolean"},
  "dispatch":{"enum":["framework","actions_shortcut","app_shortcut","app_intent"]},
  "installed":{"type":"boolean","description":"the shortcut or intent is available on this phone now"},
  "guarded":{"type":"boolean"} } }
```
- `kind` is derived: from `webResearchIntents.kind` when present; otherwise `send` for intent titles starting with Send/Reply/Post/Message/Call, `create` for Create/Add/New/Log, `open` for Open, `search` for Find/Search, `read` for Get/Show/Read, `control` for Play/Pause/Toggle/Set/Start/Stop, else `other`. A derived `kind` is a hint; the guard (section 8) decides on `kind` and the app, not on the title alone.
- Limit: Apple gives apps no list of other apps' intents, and each shortcut needs one "Add Shortcut" tap by the user. Settings shows one "Add all" list. Until added, `installed:false` and `phone.actions.run` returns `SHORTCUT_NOT_INSTALLED`.
- Apple frameworks the app calls directly (`app:"allternit"`, `dispatch:"framework"`): EventKit, Contacts, HomeKit, MusicKit, PhotoKit, HealthKit (read, log), MapKit, WeatherKit, CoreLocation, Files, pasteboard, MessageUI compose. The "Allternit Actions" shortcut uses built-in Apple actions only and dispatches on a JSON `action` field.

## 8. Approval classes

Every acting call crosses `guardCheck` (`phone-tools.ts:887`, the `GUARDED_TOOLS` set) and the runtime guard (`cmd/allternit-api/src/phone_guard.rs`). Human approval is required for:

| Class | Includes |
|---|---|
| `send` | any message, mail, post or call to another person: `phone.shortcuts.run` / `phone.actions.run` with `kind:"send"`; Messages, Mail, Slack, WhatsApp, Phone and FaceTime actions; `phone.type`+send taps in a messaging app |
| `purchase` | buying, ordering, paying, subscribing, tipping, Wallet payment |
| `security` | `phone.home.set` or `phone.home.scene` touching a lock, garage door, alarm or camera; changing passcode, Find My, Apple Account |

Read-only and `create`/`open`/`control` actions in the user's own data (notes, reminders, lights, music) are not guarded. An unknown shortcut name in `phone.shortcuts.run` is guarded (cannot know what it sends). A denied approval returns `ok:false, detail.error:"denied"`; the router MUST NOT retry it on another route. Rule-table tests in `phone_guard.rs` and the Desktop guard MUST cover every class above.

## 9. Socket messages

On the existing app-phone control lane. Files: iOS `surfaces/allternit-mobile/ios/Shared/AppPhone/AppPhoneLink.swift`; Android `surfaces/allternit-mobile/android/app/src/phoneshare/java/com/allternit/phonelink/core/AppPhoneLink.kt`; runtime `cmd/allternit-api/src/phone_app.rs`. Messages are JSON objects with a `type` string (as `input.events`, `screen.prompt` today), carried inside the existing `ictrl-v1` secure envelope (the label stays). Every request carries `id` (string, unique, echoed in the reply). Direction: runtime to phone unless noted; the phone answers `<type>.result`.

Common reply: `{ "type":"<req>.result", "id": string, "ok": boolean, "route": "official", "verified": boolean, "detail": object, "needs_you"?: { "code": NeedsYouCode, "message": string } }`. A request not answered in its `timeout_s` (default 30) is failed by the runtime with `detail.error:"timeout"`.

### 9.1 iOS app (the phone being controlled), `official`

| type | Fields | Result `detail` |
|---|---|---|
| `device.describe` | `id` | `{ foreground:boolean, locked:boolean\|null, shortcuts:string[], home:{available:boolean}, broadcast:boolean, catalog }`. `catalog`: `{ built, actions_shortcut:{ name, known, link, actions:string[], action_list }, apps:[{ id, name, category, shortcut, shortcut_known, shortcut_link, actions:int, web_actions:int, action_list }] }`; each `action_list` item is `{ id, title, kind, params:string[], opens_app?, confidence?, source:"app_intent"\|"web_research" }` (usable intents only, section 7.2) |
| `device.shortcuts.run` | `id`, `name:string`, `input?:any`, `timeout_s?:int` | `{ output:any\|null, returned_via:"app_intent"\|"x_callback" }` |
| `device.home.list` | `id`, `kind?:string` | `{ devices:[...], scenes:[...] }` |
| `device.home.set` | `id`, `device_id:string`, `state:object` | `{ id, state:object }` |
| `device.home.scene` | `id`, `scene_id:string` | `{ id, ran:boolean }` |
| `device.screen.read` | `id`, `filter?:{label?:string,type?:string}` | `{ elements:Element[], text:string, width:int, height:int }` (on-phone OCR of the latest broadcast frame; may be refused when the extension has no memory: `error:"unavailable"` with `fallback:"frame_sent"` (a fresh keyframe is on its way) or `"runtime_reads_frames"`; Desktop then OCRs a broadcast still itself and answers with the same fields plus `read_by:"desktop"`, on a Mac only. After `frame_sent` Desktop waits up to 3 s for a still taken after the request and refuses an older one (`error:"no_screen"`, `detail.error:"stale_frame"`); after `runtime_reads_frames` it reads the newest still, since the broadcast sends a frame on every screen change) |

### 9.2 HID sender messages (Android `bt-android`, second Apple device `bt-apple`)

The sending device receives these and drives its Bluetooth HID link to the target iPhone.

| type | Fields | Result `detail` |
|---|---|---|
| `hid.pair.status` | `id`, `reply`: `{ paired:boolean, connected:boolean, name?:string, keyboard_for?:string }` (`keyboard_for`, `bt-apple` only: the serial of the iPhone the person picked in keyboard-mode settings; Desktop never uses that keyboard for another iPhone) | (this one is also sent phone to runtime unprompted when state changes, without `id`) |
| `hid.pointer.move` | `id`, `dx:int`, `dy:int` (relative, -127..127 per message, sender splits larger) | `{}` |
| `hid.pointer.click` | `id`, `button:"left"\|"right"`, `down?:boolean`, `up?:boolean` (both default true) | `{}` |
| `hid.pointer.reset` | `id` (drive to the corner) | `{}` |
| `hid.key` | `id`, `usage:int` (HID usage page 0x07), `modifiers:int` (bitmask: 1 ctrl, 2 shift, 4 alt, 8 cmd), `down:boolean` | `{}` |
| `hid.type` | `id`, `text:string` (max 500, ASCII) | `{ sent:int }` |
| `hid.scroll` | `id`, `lines:int` (positive = down) | `{}` |

Errors: `ok:false, needs_you.code` `BT_NOT_PAIRED` when no paired host, else `detail.error:"hid_failed"`.

On the Android app-phone control socket these travel in the existing RPC shape: `{id, method:"hid.…", params}` answered by `{id, result}` or `{id, error:{code,message}}` (codes `bt_not_paired`, `hid_unavailable`, `hid_failed`, `invalid_params`, `bad_params`), sent by Desktop through the node's `POST /api/v1/phones/app/:id/rpc`. The phone does not send `hid.pair.status` unprompted there (the node keeps only `state` frames); Desktop asks for it. A sender MUST release all keys and buttons when the link drops or the app is backgrounded.

The Mac sender and the board are not on this lane (local to Desktop), but implement the same `HidTransport` (section 6).

### 9.3 Phone listing

The app's `state` frame carries `keyboard` while it can be a Bluetooth keyboard and mouse: iOS keyboard mode `{ on, connected, keyboard_for? }` (`keyboard_for` = the serial of the iPhone the person picked, "Which iPhone is this the keyboard for?"; picked for them when the account has one other iPhone), Android `{ on, paired, connected, host? }` (`host` = the connected iPhone's Bluetooth name). The node keeps it with the rest of the state frame.

`GET /api/v1/phones` adds to app iPhone rows: each keyboard is matched to one iPhone (iOS: the picked serial when listed; Android: the iPhone whose name equals `host`, ignoring case and curly apostrophes; else the only other app iPhone). A second iPhone or iPad in keyboard mode gets `keyboard_for`: the matched iPhone's name, or `""` when unmatched (an Android phone stays a phone and gets none). Every other app iPhone gets `routes`: the rows the node can see in the `phone.describe` row shape: `official`, then `bt-apple` / `bt-android` when a keyboard is matched to it (connected and broadcasting: healthy; not connected: `BT_NOT_PAIRED`; broadcast off: `BROADCAST_STOPPED`; two connected: not set up) or an unmatched one could be (not set up). Desktop answers the full list for the connect kit (`phones.phoneRoutes(serial)`: `{ routes, chosen }`, `chosen` = the route a tap would use now); web, the phone layout and the PWA merge it over the listed rows (Desktop's row wins per route), so every route shows on Desktop and the node's rows show everywhere.

## 10. Licensing

- Bluetooth HID code is ours. Builders MUST NOT open or copy `jqssun/darwin-bt-remote` (AGPL-3.0), `android-bt-remote` (GPL) or `sryo/clak` (no licence); they are behaviour references only, from their public descriptions.
- Reusable with a `NOTICE` entry and licence header: `mirroir-mcp` (Apache-2.0, subset vendored into `cmd/allternit-mirror`) and `Snouzy/glisse` (MIT, ESP32 dongle).
- The existing `esp32-iphone-agent` (MIT) port in `phone-usb-agent-pointer.ts` stays attributed.
- iCTRL Apache attribution headers and `ictrl-v1` wire labels stay (`phone_app_envelope.rs`, `SecureEnvelope.swift`, `H264Encoder.swift`).

## 11. Unverified until plan step 1

| Id | Question | Affects |
|---|---|---|
| P1 | Stock mirroir-mcp: screenshot, OCR, tap, type, swipe work; macOS version; whether focus is taken | `mirror` |
| P2 | A Mac can act as a Bluetooth keyboard and mouse (pairs, pointer shows, reconnects after sleep). darwin-bt-remote says yes, Glisse says no | `bt-mac`; if no, board Bluetooth mode (plan step 11) runs |
| P3 | Any Bluetooth keyboard: Cmd+Space opens an app by name, Cmd+H goes Home, Full Keyboard Access navigates | all `bt-*`, `FULL_KEYBOARD_ACCESS_OFF` detection |
| P4 | The AssistiveTouch pointer appears in broadcast and AirPlay frames; corner reset is repeatable | pointer verify and re-home, `ASSISTIVETOUCH_OFF` detection |
| P5 | The App Store app on a second iPhone or iPad can control the main iPhone as a Bluetooth keyboard/mouse | `bt-apple` |
| P6 | A hand-typed x-callback shortcut run returns a result; an App Intent can return without bringing the app forward | `official` return path, `returned_via` |

Also unverified: non-ASCII typing; on-phone OCR inside the ~50 MB broadcast extension (`device.screen.read`); accuracy of OCR versus the helper's element tree; Play policy and OEM limits on `BluetoothHidDevice`; the latency targets in section 1.1. Real-device results are reported as "not checked" until run.
