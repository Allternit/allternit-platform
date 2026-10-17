# chatgpt-android-app — ChatGPT on a linked Android phone (route `android-app`)

Drives the user's own signed-in ChatGPT app (`com.openai.chatgpt`) on a Phone-Linked Android phone through Allternit Desktop's phone gateway (`~/.allternit/phone-gateway.json`; tools `phone.open_app`, `phone.read_ui`, `phone.find_and_tap`, `phone.type`). No vendor login, cookies or tokens; nothing runs until a bot opens a chat. Shared code: `../_shared/phone/` (provider, screen parser, replay), spec: `../_shared/phone/chatgpt.ts`.

A reply is finished when the stop control is gone and the send/message control is back, with the text unchanged across reads — never "the screen stopped changing".

## Capability contract (spec §3)

| Item | State |
|---|---|
| Start a chat | Partial: opens the app on its current conversation; a fresh-chat control isn't mapped yet |
| Continue a chat | Implemented |
| Pick the model/mode | Not yet (iPhone shows the control as `Intelligence`; not driven) |
| Stream + know when finished | Implemented (polling read_ui; deltas emitted) |
| Stop mid-stream | Implemented (taps the stop control) |
| Attachments in | Not yet (needs the share sheet / file picker) |
| Text out exact | Implemented (from the UI tree, not OCR) |
| Files/images out | Not yet |
| List/reopen past threads | Not yet |
| Typed errors: signed out, usage limit, verification wall | Implemented (AUTH_REQUIRED, RATE_LIMITED, LANE_BLOCKED) |

Because items are still open, the router keeps this route **not eligible** for ChatGPT until they pass — never quietly offered with less.

## Verified
- Offline: `POST /aai/conformance/chatgpt-android-app` passes (replayed screens recorded 2026-10-17); `test/phone-adapters.test.ts`.
- Live (moto g play 2024, Android 14, 2026-10-17, through the installed Desktop's gateway): signed-out app → AUTH_REQUIRED; with that check disabled for the test, a message was sent and the reply "Allternit phone adapter works" returned in 7.8 s with opened → started → delta → completed events.
