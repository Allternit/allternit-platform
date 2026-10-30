---
name: allternit-phones
description: "Drive real phones, phone numbers and mailboxes through Allternit's MCP server. Use when the task is to operate an Android phone or iPhone (take a screenshot, tap, type, open an app, run a goal on the phone, replay a saved phone skill, run the same job on several phones), test a mobile app on a real device, give an agent its own phone number and mailbox, send or read an SMS, read a one-time code (OTP) or verification email, or share a live view link of a phone screen. Triggers on: phone, device, Android, iPhone, app on my phone, SMS, text message, OTP, verification code, 2FA code, agent number, agent mailbox, Allternit phones."
---

# Allternit phones, numbers and mailboxes

Allternit's remote MCP server (`https://mcp.allternit.com/mcp`) exposes the project owner's
linked phones, the project's phone numbers and its agent mailboxes as MCP tools. The phones
themselves are linked to the owner's **Allternit Desktop** (Phone Link, adb or a USB Agent);
every phone call is relayed to that Desktop and runs through the same phone gateway a bot uses.

## Setup check (do this first)

1. The `allternit` MCP server must be connected. If its tools are missing, tell the user to:
   - create a **project API key** at platform.allternit.com, API Keys, Platform API, with the
     scopes they need: `phones` (drive phones), `numbers` (get numbers), `messaging` (texts),
     `mail` (mailboxes). Use a key that is **not bound to an account**; phones belong to the
     owner, so an account-bound key is refused (`403 account_bound_key`).
   - export it before starting the agent: `export ALLTERNIT_API_KEY=alt_live_...`
     (`alt_test_...` is the sandbox: simulated numbers, nothing delivered).
2. Call `phones_list`. Each phone has a `serial` (`link:...`, `usb:...` or `host:port`),
   `online` and `desktop_online`. Use `-` as the serial only when exactly one phone is connected.
3. If `desktop_online` is false, or a call fails with `phone_desktop_offline`, ask the user to open
   Allternit Desktop, signed in. `phone_offline` means that phone is not connected to the Desktop.

Never print the API key, put it in a file in the repo, or paste it into a tool argument.

## Safety rules (not optional)

- **Texts, calls and reading texts need the owner.** `phone.sms.send`, `phone.call.dial` and
  `phone.sms.read` open an approval dialog on the owner's Desktop every time. You cannot skip it.
  Tell the user to watch for it; a refusal comes back as `ok: false, error: "approval_denied"`.
  Do not retry a denied action.
- **Only accounts the user authorized.** Never sign into, create, or recover an account the user
  did not ask for, and never use a phone, number or mailbox to get around another service's
  verification, rate limits or terms. Give an agent an identity only for the user's own services
  or services that allow it.
- **No surprise side effects.** Before anything that sends, buys, pays, posts, deletes, or changes
  settings on the phone, state what you are about to do and get a yes. Buying a live number is
  billed monthly; say so before calling it.
- **Personal data stays put.** History redacts typed text, numbers and message bodies. Don't
  copy screen contents, contacts or messages into code, commits or logs unless the user asks.

## Tools

| Tool | Use |
|---|---|
| `phones_list` | Linked phones and whether they are reachable |
| `phone_screenshot` | Current screen (image plus screen text when available) |
| `phone_tap` | Tap at `x`, `y` in screenshot pixels |
| `phone_type` | Type text into the focused field |
| `phone_open_app` | Open an app by name or package / bundle id |
| `phone_task` | Give the phone a goal; it loops screen, action, screen on the Desktop. Returns a `run_id` |
| `phone_run_status` | Status and steps of a task or skill run (`run_id`) |
| `phone_skill_run` | Replay a saved phone skill with its `inputs` (no model needed) |
| `phone_history` | Recent steps on a phone, optionally one `run_id` |
| `phone_live_url` | A link that shows the phone screen live |
| `mailbox_create` / `mailbox_list` | Agent mailboxes |
| `mail_list` / `mail_read` | Messages in a mailbox |
| `mail_read_otp` | The newest one-time code in a mailbox |

Check each tool's input schema before calling it; argument names come from the server.
For anything the tools don't cover, the same project key works on the REST API at
`https://api.allternit.com/v1` (`/v1/phones`, `/v1/phones/runs`, `/v1/numbers`, `/v1/messages`,
`/v1/mailboxes`, `/v1/identities`).

## Workflows

### 1. Screenshot, then tap (manual driving)

1. `phone_screenshot`. Read the screen; find the target by its text, not by guessing coordinates.
2. `phone_tap` at the centre of the element (screenshot pixels).
3. `phone_screenshot` again to confirm the screen changed as expected. Never chain several taps
   blind; one action, one check.
4. For text: tap the field, `phone_type`, screenshot to confirm.
Use this for short, precise steps and for checking a build of the user's own app.

### 2. Run a task (let the phone loop)

For anything over about five steps, prefer `phone_task` with a plain goal
("Open Settings and turn on Wi-Fi"), `max_steps` 1 to 40 (default 15). It answers at once with a
`run_id`; poll `phone_run_status` every few seconds until it is done (a task stops after 5 minutes),
then report what happened from its steps. Tasks and skill replays never text or call.
If a saved skill fits (ask the user, or check with the REST `GET /v1/phones/skills`), use
`phone_skill_run` instead: faster, no model, deterministic. Pass every input the skill lists.

### 3. Fleet run (same job on several phones)

Use REST `POST /v1/phones/runs` with the goal or skill and the serials (see the Phones docs), or
start one `phone_task` per serial. Track each `run_id` with `phone_run_status` and give the user a
per-phone table: serial, status, steps, failure reason.

### 4. Give an agent an identity

An identity is a number, a mailbox and (optionally) a phone for one agent or test persona.
- One call: REST `POST /v1/identities` creates the number and mailbox together and returns both.
- Or step by step: `POST /v1/numbers` (scope `numbers`), then `mailbox_create`.
In a sandbox project numbers are simulated `+1 555` numbers that never reach a carrier. Live
numbers need a paid plan and are billed monthly; confirm with the user first. US texting from a
live number needs carrier registration (`/v1/numbers/{id}/registration`).

### 5. Read a one-time code

- **Email:** `mail_read_otp` on the mailbox the service sent to. If it returns nothing, wait a few
  seconds and retry (up to about a minute), then `mail_list` to see what did arrive.
- **SMS to an Allternit number:** REST `GET /v1/messages?number_id=...&direction=inbound`, newest first.
- **SMS on the user's own phone:** `phone.sms.read` through the phone tools. This needs the owner's
  approval on their Desktop every time; tell them before you ask.
Use a code only for the sign-in the user asked for, then stop. Don't store it.

### 6. Share a live view

`phone_live_url` returns a link to watch the phone screen. Give the link to the user as-is; it is
tied to their account. Mention it when starting a long `phone_task` so they can watch.

## Errors

| Code | What to do |
|---|---|
| `phone_desktop_offline` | Ask the user to open Allternit Desktop, signed in |
| `phone_gateway_unavailable` | Desktop is up but its phone gateway is off; ask the user to restart it from Desktop |
| `phone_offline` | That phone isn't connected; pick another serial or ask the user to reconnect |
| `phone_required` | Several phones connected; pass a serial instead of `-` |
| `phone_timeout` | No answer in time; a text or call may still be waiting for approval |
| `no_desktop` | The owner never paired a Desktop; they need to install and sign in |
| `403 permission_error` | The key lacks a scope; ask the user to add it to the key |
| `accessibility_off` | The phone's accessibility service is off; the user turns it on in Phone Link |

Docs: https://docs.allternit.com/api/platform/sdks/claude-code-codex and
https://docs.allternit.com/api/platform/phones
