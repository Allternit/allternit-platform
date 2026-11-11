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
