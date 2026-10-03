# Third-Party Notices

This file contains notices for open-source software incorporated into this product.

---

## GenOffice
**License:** Apache-2.0

**Copyright:** Copyright 2026 Mainfunc, Inc.

**Source:** https://github.com/genspark-ai/genoffice

**Used in:**
- `platform/packages/office-docx-engine/`
- `platform/packages/office-pptx-engine/`
- `platform/packages/office-pptx-render/`
- `platform/packages/office-file-parse/`
- `platform/packages/office-xlsx-engine/` (Rust crate from `apps/sheets/native/xlsx-engine` + Node sidecar client)

**GenOffice document office engines forked and adapted for the Allternit workspace. Each package retains a copy of the GenOffice `LICENSE` and `NOTICE` files.

---

## anydoc
**License:** MIT

**Copyright:** Copyright (c) 2026 Firecrawl

**Source:** https://github.com/firecrawl/anydoc

**Used in:** `services/office-engine/` (`@firecrawl/anydoc` npm package, pinned at `0.1.6`)

**Rust document → GFM Markdown converter with napi prebuilt Node bindings; powers the office engine's `POST /markdown` endpoint.**

---

## Readability
**License:** Apache-2.0

**Copyright:** Copyright (c) 2023 Mozilla

**Source:** https://github.com/mozilla/readability

**Used in:** `services/office-engine/` (`@mozilla/readability` npm package, `0.6.0`)

**Main-content extraction for the office engine's `POST /markdown-url` endpoint (URL → Markdown).**

---

## herdr (ao engine)
**License:** Apache-2.0

**Copyright:** Copyright (c) herdr contributors

**Source:** https://github.com/herdrdev/herdr (tag v0.9.0, commit b99002ac99b09e00b4ca692436cb15a6b0d676f1)

**Used in:** `infrastructure/executor/ao-engine/`

**Terminal workspace engine for AI coding agents, vendored as the `ao` engine fork (internal crate name `herdr` retained for upstream mergeability). Upstream is Apache-2.0 since 0.8.0; the herdr.dev self-updater, product announcements, and remote manifest fetches were removed per the P0 gut list (no runtime upstream URLs remain). LICENSE preserved in-crate.**

---

## Turndown
**License:** MIT

**Copyright:** Copyright (c) 2017 Dom Christie

**Source:** https://github.com/mixmark-io/turndown

**Used in:** `services/office-engine/` (`turndown` npm package, `7.2.0`)

**HTML → GFM Markdown conversion for the office engine's `POST /markdown-url` endpoint.**

---

## linkedom
**License:** ISC

**Copyright:** Copyright (c) 2020-present Andrea Giammarchi

**Source:** https://github.com/WebReflection/linkedom

**Used in:** `services/office-engine/` (`linkedom` npm package, `0.18.12`)

**Server-side DOM for Readability in the office engine's `POST /markdown-url` endpoint.**

---

## Kimi Code
**License:** MIT

**Copyright:** Copyright (c) 2026 Moonshot AI

**Source:** https://github.com/MoonshotAI/kimi-code

**Used in:** `cmd/gizzi-code/src/runtime/agents/adaptive-run-batch.ts`

**Adaptive agent batch scheduling adapted to Allternit's launcher contract.**

---

## open-cowork
**License:** MIT  
**Used in:** `platform/packages/cowork-engine/src/scheduler/`, `platform/packages/cowork-engine/src/memory/`, `platform/packages/cowork-engine/src/sandbox/`  
**Portions ported to TypeScript with Prisma/async adaptations.**

---

## mem0
**License:** Apache-2.0  
**Source:** https://github.com/mem0ai/mem0  
**Used in:** `tools/cowork-integration/stack/services/memory/`  
**Docker sidecar; REST and MCP interfaces on port 8765; Qdrant vector backend.**

---

## mcp-memory-service
**License:** Apache-2.0  
**Used in:** `tools/cowork-integration/stack/services/memory-mcp/`  
**Docker sidecar on port 8761; sqlite_vec backend with WAL mode.**

---

## eigent
**License:** Apache-2.0  
**Used in:**  
- `surfaces/allternit-platform/src/views/cowork/components/WorkflowPipeline.tsx` (adapted from `src/components/WorkFlow/`)  
- `surfaces/allternit-platform/src/views/cowork/components/BrowserAgentWorkspace.tsx` (adapted from `src/components/BrowserAgentWorkspace/`)  
**ReactFlow multi-agent pipeline and screenshot workspace UI; Electron dependencies removed.**

---

## cline
**License:** Apache-2.0  
**Source:** https://github.com/cline/cline  
**Used in:** `platform/packages/cowork-engine/src/approval/`  
**ApprovalGate class; auto-rules engine with timeout.**

---

## hermes-agent
**License:** MIT  
**Used in:** `cmd/allternit-api/src/cowork/scheduler.rs`  
**Cowork task scheduler; wired into allternit-api main.rs.**

---

## CoWork-OS
**License:** MIT  
**Used in:**  
- `cmd/allternit-api/src/cowork/executor.rs` (SubAgentOrchestrator)  
- `cmd/allternit-api/src/cowork/background_service.rs` (subconscious background loop)  
- `domains/cowork/connectors/linear/`, `connectors/jira/`, `connectors/google-workspace/`, `connectors/hubspot/`, `connectors/figma/`, `connectors/asana/`, `connectors/salesforce/`, `connectors/zendesk/`, `connectors/okta/`, `connectors/monday/`, `connectors/discord/`  
**MCP stdio connector servers; CRUD operations for each platform.**

---

## OpenSandbox
**License:** Apache-2.0  
**Used in:** `tools/cowork-integration/stack/services/sandbox/`  
**Docker sidecar on port 8762; opensandbox/server image with Docker socket mount.**

---

## browser-use
**License:** MIT  
**Source:** https://github.com/browser-use/browser-use  
**Used in:** `tools/cowork-integration/stack/services/browser-agent/`  
**Docker sidecar on port 8763; MCP stdio interface for browser automation.**

---

## agent-s (GUI Agents S2)
**License:** Apache-2.0  
**Used in:** `domains/computer-use/core/adapters/hybrid/orchestrator/`  
**Hybrid orchestrator for computer-use; integrated into ACU build.**

---

## DeerFlow
**License:** MIT  
**Source:** https://github.com/bytedance/deer-flow  
**Used in:** `tools/cowork-integration/stack/services/research/`  
**LangGraph-based research super-agent; gateway on port 8764.**

---

## AionUi
**License:** Apache-2.0  
**Used in:** `platform/packages/cowork-engine/src/sub-agent/`  
**TeamSession and AgentFactory; concurrency cap and HTTP sub-agent runner.**

---

## agent-zero
**License:** MIT  
**Used in:** `platform/packages/cowork-engine/src/personas/`  
**CoworkPersonaStore with built-in persona definitions.**

---

## HarnessRouter Community Edition
**License:** Apache-2.0

**Copyright:** HarnessRouter contributors (no explicit copyright line in LICENSE; standard Apache-2.0 text with upstream `NOTICE` file)

**Source:** https://github.com/HarnessRouter/harnessrouter (default-branch HEAD, commit `60c9a4556a4dd9e04dff28ad8404b006747c2204`, vendored 2026-09-10)

**Used in:** `vendor/harnessrouter-ce/`

**Reference + test-oracle material ONLY for the UHP (Unified Harness Protocol) Rust server work: protocol spec markdown, JSON Schema + OpenAPI documents, the `uhp-conformance` check suite, and pytest layouts. No code from this tree is shipped, compiled, or executed by this repo; it is not part of the cargo workspace. Full `LICENSE` preserved at `vendor/harnessrouter-ce/LICENSE`. Inventory: `docs/programs/ao/UHP_VENDOR_INVENTORY.md`.**

---

## Jev Showcase (cobusgreyling/Jev)
**License:** MIT

**Copyright:** Copyright (c) 2026 Jev Showcase contributors

**Source:** https://github.com/cobusgreyling/Jev

**Used in:** `tools/system-one-local/skills/` (`jev`, `jev-fanout`, `jev-guardrail`, `jev-route`)

**Agent-skill markdown adapted (rewritten to point at the local System One server first, with Allternit FORBIDDEN blocks added). No code copied. MIT license text reproduced in `tools/system-one-local/skills/NOTICE.md`. The System One request/response contract itself follows TypeSafe's public API docs (docs.typesafe.ai); no TypeSafe code is included.**

---

## Scratch / Original (Allternit)
The following were written from scratch for this product:
- `domains/cowork/connectors/slack/`
- `domains/cowork/connectors/github/`
- `domains/cowork/connectors/notion/`
- All platform API routes under `surfaces/allternit-platform/src/app/api/v1/cowork/`
- `platform/packages/cowork-engine/src/memory/service.ts`

---

## Allternit voice (services/voice, services/voice-tts)

**allternit-voice-service** (speech-to-text and the voice HTTP API) links
[sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) (Apache-2.0) and
onnxruntime (MIT) statically. It contains no GPL code; this is checked at
build time by `scripts/check-voice-no-gpl.sh`.

**allternit-tts** (text-to-speech, a separate program that
allternit-voice-service starts as a child process) is licensed
**GPL-3.0-or-later**, because it links
[espeak-ng](https://github.com/espeak-ng/espeak-ng) (GPL-3.0-or-later)
through sherpa-onnx's Kokoro frontend. The full licence text ships as
`licenses/allternit-tts-GPL-3.0.txt`. Its complete corresponding source is
`services/voice-tts/` in the Allternit platform repository, built against
sherpa-onnx 1.13.8 (https://github.com/k2-fsa/sherpa-onnx/releases/tag/v1.13.8,
which carries the espeak-ng and piper-phonemize sources it builds).

Voice models download on first use and are not part of the installer:
Silero VAD (MIT), Moonshine (MIT), Smart Turn v3.2 (BSD-2-Clause),
Kokoro-82M (Apache-2.0) and, optionally, Parakeet TDT 0.6B v3 by NVIDIA
(CC-BY-4.0). Full list: `services/voice/THIRD_PARTY_NOTICES.md`.
