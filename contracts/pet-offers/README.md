# Pet offers contract (v1)

The bot's pet notices an error, offers help in a bubble, and acts only after a tap.
`pet-offers-v1.json` is the one source every surface reads for that loop:

- **Offer shape:** `{id, surface, app, appId, title, windowKey, trigger:{kind, ruleId, line}, text, botId, createdAt}`.
  `text` stays on the device until the user accepts. It never goes in a notification, a log or a URL query string.
- **Kinds:** `error_dialog`, `stack_trace`, `build_failed`, `run_failed`. `run_failed` comes from Allternit's own failure
  events, not from a text rule.
- **Copy:** one line per kind, the "Fix it" and "Mute {App}" links, and the generic notification line.
- **Rules:** regexes checked in file order; the first match wins. Rules with `scope: "dialog"` only apply to text from a
  dialog window (see `dialogRoles`). `matcher.steps` spells out the exact algorithm each surface ports.
- **Block defaults:** password managers, banking apps and hosts, private-window titles, Messages apps. Secure text fields
  are never read.
- **Settings:** keys and defaults, stored on the device only.
- **`turnOptions.computer`:** the per-turn `metadata.tools.computer` option a Desktop offer can set.
- **Fixtures:** sample texts with the kind each one must produce (or `null`). Every surface's matcher must pass all of them.

## The one switch to flip

`settings.defaults["petOffers.readOtherApps"]` decides whether Desktop reads other apps' windows before the user opts in.
It is `false` today. Change it there, run the generator, and every surface picks it up.

## Regex rules must stay portable

Patterns run in JS (Desktop, web, Gizzi), ICU (iOS `NSRegularExpression`) and `java.util.regex` (Android).
So: no lookbehind, no named groups or backreferences, no inline flags, no `\p{...}`, no atomic or possessive groups,
no `\A \Z \z`, ASCII only (write `’` for a curly apostrophe). Flags are only `i` and `m`. The generator rejects the rest.

## Commands

```sh
node contracts/pet-offers/generate.mjs           # write the generated files
node contracts/pet-offers/generate.mjs --check   # exit 1 if any generated file is stale
node contracts/pet-offers/generate.mjs --test    # run every fixture through the rules (JS RegExp)
```

Generated files (never edit by hand):

| Surface | File |
|---|---|
| Desktop main | `surfaces/allternit-desktop/src/main/pet-offers.gen.ts` |
| Gizzi TUI | `cmd/gizzi-code/src/cli/ui/ink-app/pet/petOffers.gen.ts` |
| iOS | `surfaces/allternit-mobile/ios/Core/PetOffers/PetOfferContract.gen.swift` |
| Android (Phone Link) | `surfaces/allternit-mobile/android/app/src/phonelink/java/com/allternit/phonelink/core/PetOfferContract.gen.kt` |

The web, phone and PWA code lives in the `allternit-ai` repo, which keeps a byte copy of the JSON at
`src/lib/pet-offers/pet-offers-v1.json`. When the JSON changes here, copy it there in the same change.

## Computer tools per turn

A Desktop offer accepted with "computer tools on" sends `metadata.tools.computer: true` on that one turn
(`turnOptions.computer` in the JSON). The path, traced on 2026-11-11:

1. **allternit-api** forwards `metadata.tools` whole (`stored_turn_metadata`, `cmd/allternit-api/src/v1_routes.rs:963`).
   Nothing else in the send body reaches the stored message. Test:
   `stored_turn_metadata_passes_the_computer_option_through`.
2. **gizzi runtime** parses it into `MobileTools.computer` (`cmd/gizzi-code/src/runtime/session/prompt.ts`,
   `parseMobileToolOptions`) and passes `{ computer }` to `ToolRegistry.tools(model, agent, sessionID, turn)`
   (`cmd/gizzi-code/src/runtime/tools/builtins/registry.ts`). The toolset (`computer`, `computer_v2`, `computers`)
   is listed when `GIZZI_ENABLE_COMPUTER_TOOL` is set **or** the turn carries `computer: true`. `computer: false`
   never removes what the env flag turned on. Other `ToolRegistry.tools` / `ids` callers pass nothing and are unchanged.
   Nothing is persisted; the next turn without the key has no computer tools. Older runtimes drop the unknown key.

### Do read-only members need the control lease?

Yes. The lease gate runs for **every** member of the `computer` toolset, read-only ones included (`screenshot`,
`zoom`, `read_ui`, `verify`). Only the `browser` toolset skips it.

- Gate call: `cmd/allternit-api/src/computer_toolset.rs:1914-1921` (`if req.toolset == Toolset::Computer`), before
  policy, safety and dispatch. `computer_v2` calls go out as toolset `computer`, so they are gated the same way.
- Rule: `lease_gate`, `computer_toolset.rs:1820`. On **this device** the call passes only while the owner (or this
  agent's run) holds control; the agent never takes it by itself. On other computers the agent takes or renews the
  lease unless someone it can't preempt holds it.
- So the first computer call of an accepted offer, even a screenshot, is refused until the person takes control.
  That is the "asks before its first step" behaviour. Nothing takes the lease automatically.

### How a 423 surfaces

`lease_check` (`computer_toolset.rs:2190-2213`) answers HTTP 423 with `error` set to one of:

| `error` | When | Message the model and the person see |
|---|---|---|
| `take_control_first` | This device, nobody holds control | "Take control of this computer first (Computers > This computer > Take control) so the agent can use its mouse and keyboard." |
| `computer_controlled_elsewhere` | Someone else holds control | "{who} is controlling this computer. Ask them to hand over control." |

A `run_subtask` / `run_skill` already running on the same computer answers 423 `computer_busy` (`computer_toolset.rs:2091`).

In gizzi, `runToolsetCall` (`cmd/gizzi-code/src/runtime/tools/computer-toolset/index.ts`) treats any status of 400 or
more as a failed tool call: it throws `Error(message)`, so the tool part ends in `status: "error"` with the message
text above. **The `error` code is not passed to the tool part**, only the text. No ai-repo code handles these codes
yet; the HUD take-control prompt (step U2) has to key on the tool part's error text (or a later runtime change can
add the code to the part metadata).

### Money, send and delete still ask first

`computer_safety::assess` (`cmd/allternit-api/src/computer_safety.rs:1034`) classifies every step. Irreversible
classes (send, pay, purchase, transfer, delete, publish, credentials, account; `computer_safety.rs:72-97`, phrases at
`:100-140`) become `Verdict::Confirm` (`:1099-1126`). The executor then refuses the step with 409
`approval_required` and `risk: "irreversible"` until a single-use grant is attached
(`computer_toolset.rs:1991-2043`). This gate is server-side and does not care how the tool got turned on, so the
per-turn option adds nothing to it and needs nothing new.

One limit, in the runtime: gizzi answers that 409 with `ctx.ask(permission: "computer_toolset.computer")`
(`computer-toolset/index.ts`, `runToolsetCall`) and does not mark the ask `destructive`. So in a turn or session whose
permission mode is `bypassPermissions` (composer "always") or `GIZZI_SKIP_PERMISSIONS`, and in app-session `auto` mode,
`PermissionNext.evaluatePolicy` (`cmd/gizzi-code/src/runtime/tools/guard/permission/next.ts:624-660`) approves it
without showing the person. In `default` mode the person is asked. A one-line runtime fix is to pass
`destructive: reply.body.risk === "irreversible"` in that ask's metadata (auto mode then asks; bypass still skips by
design).
