# agent-modes contract

`agent-modes-v1.json` is the one list of agent modes. The composer deck's Agent mode drop-up, the Artifacts page,
the rail "+" menu and the native pickers all read it, in this order and with these icons:

Agent (auto), Agent Swarm, Docs, Sheets, Slides, PDF, Page, Dashboard, Ops sheet, Design, Image, Video, Motion, App, Build, Deep Research.

Beta: Dashboard, Ops sheet, Design, Motion. New: App.

## Fields

| Field | Meaning |
|---|---|
| `id` | Registry id (new). Not a wire id |
| `label`, `icon`, `beta`, `new`, `placeholder` | What every surface draws. Placeholder is the composer text once the mode is picked |
| `artifact` | `true` for the 14 kinds the Artifacts page lists; `false` for the working styles Agent and Agent Swarm |
| `launch` | `agentMode` (set the mode only), `session` (start an artifact session of that kind), or `pick` (PDF opens a file picker). `agentMode` keeps today's wire id (`website`, `data`, `artifact`, ...), so stored sessions and mode contracts do not change |
| `libraryKind` | Artifacts v2 kind a result is filed under (`null` for working styles) |
| `surfaces` | Where the mode shows (`chat`, `bot`, `code`, `browser`, `design`), copied from today's `SURFACE_MODES`. A surface shows only the modes it can run |
| `can` | The "It can" toggles the kind can honour, each with its default. A toggle that is not listed is not shown. Keys: `connectors` (Use my tools), `storage` (Keep its own data), `ai` (Ask AI while open) |

`legacyIds` maps a stored id to its registry id (`artifact` reads as `agent`). `legacySurfaces` maps `cowork` to `chat`.

## Commands

```sh
node contracts/agent-modes/generate.mjs           # write the generated files
node contracts/agent-modes/generate.mjs --check   # exit 1 if any generated file is stale
```

Generated files (never edit by hand):

| Surface | File |
|---|---|
| Gizzi | `cmd/gizzi-code/src/runtime/agent-modes/agentModes.gen.ts` |
| iOS | `surfaces/allternit-mobile/ios/Core/AgentModes/AgentModes.gen.swift` |
| Android | `surfaces/allternit-mobile/android/core/api/src/main/java/com/allternit/core/api/gen/AgentModes.gen.kt` |

The web, phone and PWA code lives in the `allternit-ai` repo, which keeps a byte copy of the JSON at
`src/lib/agent-modes/agent-modes-v1.json`. When the JSON changes here, copy it there in the same change.
