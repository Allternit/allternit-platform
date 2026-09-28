# Subscription Fabric — Platform Surfaces Plan (P5/P6)

Written 2026-09-28 from three read-only code maps (gizzi-code, allternit-api,
allternit-ai UI; file:line refs below were true at origin/main that day —
re-verify before editing). Governing decisions: **D15** (runtime only on the
Sessions machine, never the user's desktop), **D16** (provider-terms
disclosure + human-initiated send), D9 (publish gate), D13 (picker catalog).
Owner direction: "called like any other model in the model selector … stream
it, show responses and everything correctly with no lag or errors"; gizzi's
tool belt + a gateway tool server; adapters driven by
`CAPABILITY_INVENTORY.md`.

## 1. Core design: the fabric is a gizzi-code provider, not a parallel pipeline

allternit-api never calls providers; every model turn runs through gizzi-code
(`/session/{id}/message`) and streams back over allternit-api's SSE bridge. The
chat picker lists what gizzi's `/provider` reports. So:

- **gizzi-code gets a `subs` provider family** (`subs-chatgpt`, `subs-claude`,
  `subs-kimi`, models = the account's model classes/ids from the gateway D13
  catalog). It appears in every picker automatically and streams through the
  existing bridge → UI path. No second streaming pipeline.
- **Its LanguageModel** is a new `SubscriptionFabricLanguageModel`
  (LanguageModelV2) patterned on `SubprocessLanguageModel`, mapping gateway
  events → AI SDK stream parts.
- **Artifacts** (images/pptx/docx/…) come back as `file` parts on the
  assistant message and as artifact frames to the UI.
- **Capability tools** (image.generate, presentation.create, research.deep …)
  go in gizzi's tool belt, human-confirmed (D16); a gateway MCP server exposes
  the same to other agents.

## 2. Seams found (and the gaps each surface must close)

### gizzi-code (`cmd/gizzi-code/src/`)
- `runtime/providers/provider.ts` — `Provider.Model` (L91–156: capabilities
  incl. output image/pdf, limits), `Provider.Info` (L160–177, `auth_type`).
  `getLanguage` (L794–831) + `resolveRuntimePolicy` (L964–992): only
  subprocess or SDK today → **add a `fabric` runtime branch**.
- `runtime/providers/adapters/loaders/subprocess.ts` — the pattern
  (event → text/reasoning parts, raw `__gizzi` parts, usage estimate).
  **Gaps to NOT copy:** sends only the last user text (fabric: use the gizzi
  session id as the fabric `thread_id` → `chat.create` then `chat.continue`,
  the P3 thread mapping), and ignores `abortSignal` (fabric: `POST
  /v1/tasks/:id/cancel`).
- `runtime/providers/discovery/index.ts` — `Discovery.register(hook)` (L77),
  `source: "subscription"` label exists. Discovered models are forced
  text-only (`provider.ts` L533–541) → **allow image/file output for subs**.
- `session/processor.ts` — `file` stream parts hit `default: unhandled`
  (L740) → **add `case "file"` → FilePart on the assistant message**
  (`message-v2.ts` L153–160 already has FilePart).
- Tool belt: `Tool.define` + `ctx.ask` (`runtime/tools/builtins/tool.ts`);
  `media-generate.ts` already has `lane: "subscription"` stubbed (L118–133)
  → **wire it to the fabric**. Registry `runtime/tools/builtins/registry.ts`.
- **HITL gap:** `PermissionNext.evaluatePolicy`
  (`runtime/tools/guard/permission/next.ts` L323–375) auto-allows in
  `auto`/`yolo`/`bypassPermissions` → D16 needs a permission class that is
  **always ask** regardless of mode (`permission: "subscription"`), answered
  via `POST /permission/:id/reply`.
- MCP sources: config `mcp` (`config.ts` L1406–1419) — the gateway MCP server
  plugs in as `{type:"remote", url, headers}`.
- Existing media registry `GET/POST /provider/media/...`
  (`routes/provider.ts` L190–257, `runtime/providers/media/registry.ts`) —
  natural home for fabric image/slides/doc/sheet/website lanes.

### allternit-api (`cmd/allternit-api/src/`)
- Picker: UI calls **`GET /api/v1/providers`** (`provider_routes.rs:876`, live
  gizzi discovery) — NOT `/api/v1/models` (that's only agent wizards).
  `available_model_catalog()` first entry is the global default model — do not
  prepend subs entries there.
- Live chat: `v1_routes::agent_chat_bridge` (`v1_routes.rs:878`, mounted
  `main.rs:1006`) → gizzi `POST /session/{id}/message`. Frames:
  message_start / content_block_delta (text|thinking) / tool_use start /
  tool_result / tool_permission / context_usage / error / finish. **No frame
  for gizzi `file` parts → add one** (the UI already parses `artifact` frames).
  No total timeout on this path (good for 5–25 min tasks).
  `chat_routes.rs` `/agent-chat` + `gizzi_chat_stream.rs` are dead code.
- `/v1/chat/completions` (`llm_gateway/proxy.rs`): **120 s hard timeout**
  (L69) + 135 s client timeout; no reasoning/file streaming; `gizzi_bus.rs`
  drops events on lag (cap 512) → subs models must not route through it (or
  it needs long-task handling).
- **Reaching the gateway:** computer proxy `ANY /api/v1/computers/:id/proxy/*`
  (streams SSE since #873; 30 s timeout on non-SSE). Recommended: a thin
  allternit-api route `/api/v1/subscriptions/*` that resolves the user's
  Sessions computer + injects the gateway token server-side, so gizzi and the
  UI never hold provider/gateway secrets. (Today a Mac skill script reads the
  Desktop token from process env — a stopgap, not product.)
- Approvals: `cowork_approvals` + `tool_permission` frame + relay to gizzi
  `/permission/:id/reply` (`v1_routes.rs:1675–1716`, `cowork_routes.rs`
  `decide_approval` :2185) — reuse for D16 human-send cards.
- Disclosure ack storage: `user_cowork_preferences` (V82 +ALTER pattern,
  `GET|PUT /api/v1/cowork-preferences`) or a small dedicated table
  (`subs_disclosure_acks(user_id, provider, version, acknowledged_at)`) —
  next migration after `V193`.

### allternit-ai UI (`/Users/joe/Desktop/allternit-workspace/allternit-ai/src/`)
- Picker: `components/chat/ModelPickerPopover.tsx` (`choose()` L387–404 =
  disclosure hook point; outside-click closes at L242–246 → portal the modal
  and defer the pick). Also guard `views/bots/BotModeModelPicker.tsx`
  `handlePickModel`. Data: `hooks/use-available-brain-models.ts` →
  `/api/v1/providers`; provider kinds from `lib/providers/provider-registry.ts`
  (add a `subscription` kind + badge).
- Streaming: `lib/agents/native-agent-api.ts` `streamChat` (~833–1060) →
  `lib/agents/mode-session-store.ts` `applyDelta` (rAF-batched). Gaps for
  subs: **no timeout/heartbeat/reconnect/resume** (5–25 min tasks need resume
  via gizzi `/event?sinceSeq`), **no progress events**, **artifacts only go
  to the canvas, never become message parts** (`onArtifact` 1711–1719),
  history `file` parts render as text `[File name]`
  (`message-ui-parts.ts:67–68`), `file` UI parts render null
  (`UnifiedMessageRenderer.tsx` default).
- Artifacts: `ArtifactCard` / `ArtifactSidePanel` kinds lack
  `slides|audio|video` and office previews; office IO exists in
  `views/documents/office-io/{docx,pptx,xlsx}.ts` to build previews from.
- Approvals UI: `views/chat/components/ChatThreadInlineGate.tsx` +
  `ChatApprovalCard`/`ChatQuestionCard` (poll 5 s) — reuse for human-send and
  provider-question cards (D16). Bug: `rust-stream-adapter.ts`
  `normalizeStreamEvent` drops `tool_permission` (L1451–1657); its `artifact`
  handler appends instead of updating by id (L1266).
- Long-task UX: `ProcessBlock` / `SmoothOrb` / `ActivityIndicator` exist;
  `task` part ignores `progress` — extend for fabric progress.
- Replies packages (`types/replies-contract.ts`, `lib/replies-reducer.ts`) are
  **unused** — don't build on them.

## 3. Build order (each a reviewable PR, live-verified on the Sessions machine)

1. **allternit-api `/api/v1/subscriptions/*` forwarder** (Sessions computer
   resolution + server-side gateway token + disclosure/initiated_by
   enforcement + ack table migration). Gateway: `initiated_by` required,
   `403 disclosure_required`.
2. **gizzi-code `subs` provider** (discovery hook from the gateway catalog via
   the forwarder; `SubscriptionFabricLanguageModel` with thread mapping,
   cancel, progress→reasoning/status, artifacts→file parts; processor `file`
   case). Verify: `subs/chatgpt:*` appears in `/api/v1/providers`, a chat turn
   streams token-by-token end to end, continue follows the thread.
3. **Bridge + UI streaming correctness:** `file`/artifact frames → message
   parts + ArtifactCard; progress parts; resume/reconnect for long tasks;
   fix the two adapter bugs; no layout jumps.
4. **Disclosure modal + ack** (picker + bot picker), **human-send / question
   cards** (always-ask permission class in gizzi; cowork approval relay).
5. **Tool belt + MCP:** wire `media_generate` subscription lane; capability
   tools (`presentation.create`, `document.create`, `research.deep`, …)
   human-confirmed; gateway MCP server for other agents.
6. **Adapters:** claude-web → kimi-web (kimi.ai) → chatgpt-web expansion, per
   `CAPABILITY_INVENTORY.md` (probe the live UI on a copy profile first; owner
   logs in once per provider through Desktop's writable computer viewer).
7. **Desktop Settings → Sessions Computer panel** (status, accounts,
   needs-user queue, login viewer).
