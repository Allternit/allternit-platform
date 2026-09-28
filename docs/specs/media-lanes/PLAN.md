# Image & video generation in Allternit sessions — three lanes

**Status:** plan for Eoj's review · 2026-09-27 · owner: Claude session joe-58
**Asked for (Eoj, 2026-09-27):** Image and Video modes must actually make images and videos in an
artifact session, through (1) the Allternit cloud subscription's models, with an upsell when the
sub is off, (2) the user's own connected subscriptions (Subscription Fabric), and (3) natively in
our harness with any model, by writing code that renders the image or video. Lane 3 is required.

## 1. What exists today (checked on `origin/main`)

| Piece | Where | State |
|---|---|---|
| API media plane | `cmd/allternit-api/src/media/` · `/api/v1/media/{catalog,image/generate,video/jobs,artifacts}` | gpt-image-2 and fal FLUX schnell (images), MiniMax H3 and fal Seedance 2.0 (video). Keys are BYOK (the V134 credential store) or a platform-funded lane that ships **off** (`ALLTERNIT_MEDIA_PLATFORM_FUNDED`). Video jobs are async; bytes are stored server-side. |
| gizzi media registry | `cmd/gizzi-code/src/runtime/providers/media/` · `/media/:mode/generate` | Pollinations (free), Bonsai local, OpenAI Image, MiniMax. HTTP routes only. |
| App Image / Video modes | `allternit-ai/src/lib/agents/modes/{image,video}-generation.ts` | Call the API media plane, but **only in the local-executor path**. A session agent (gizzi) has no media tool, and the Image and Video mode contracts require `image_generation` / `video_generation`, which no agent can call. |
| Cloud (platform console) | `cmd/allternit-cloud-api` | Routes chat models through OpenRouter and Together; Stripe plan entitlements (`hosted_entitlements.rs`). **No media.** |
| Connectors | `services/open-connector/src/providers/{higgsfield_ai,fal_ai,minimax}` | Per-user API-key actions (Higgsfield: `platform.higgsfield.ai`, key + secret). |
| Subscription Fabric | `docs/specs/subscription-fabric/`, `platform/packages/subscription-fabric-contracts` | Contracts plus a gateway with a keychain; the capability taxonomy covers image, website, document and more. P3 cloud gate in progress (Kimi's program). |
| Code-to-video | `~/.claude/skills/motion-reel` | Deterministic canvas `draw(t)` → headless Chrome frames → ffmpeg MP4, plus a numpy score. It works well, but it is a Claude Code skill, not a harness plugin. |

**OpenRouter now covers both media types** (checked 2026-09-27), so the cloud lane doesn't need a
separate ChatGPT Image integration:
- Images: `POST /api/v1/images` — `openai/gpt-image-1`, `openai/gpt-image-2`, Seedream 4.5,
  Gemini 2.5 Flash Image, FLUX 2 Pro, Recraft (SVG).
- Video: `POST /api/v1/videos` (async, 202 → poll `GET /api/v1/videos/{id}` → `content`) — Veo 3.1,
  Seedance 2.x, Sora 2 Pro, Wan 2.7, Kling, Hailuo 3. Priced per video-second.
- **Higgsfield is not on OpenRouter.** It needs its own client: `platform.higgsfield.ai`, key + secret,
  async request/status. Its catalog has 50+ models, including Soul and DoP.
- **OpenAI GPT Image 2.5 is not on OpenRouter** (Eoj, 2026-09-27). It shipped in the API on
  2026-09-08 as `gpt-image-2.5-flare` (fast, everyday production) and `gpt-image-2.5-sunburst`
  (editing precision), and it is better than gpt-image-2. It needs a direct OpenAI client. The local
  API media plane already has one for gpt-image-2 (`media/clients.rs`), which extends to the 2.5 ids.

## 2. The shape: one tool, lanes behind it

The session agent gets **one tool**, `media_generate` (gizzi builtin), used by every model:

```
media_generate({ kind: "image" | "video", prompt, size?, aspect?, duration?, references?, lane? })
  → { status: "done", artifact: {id, url, mime}, lane, model, cost? }
  | { status: "running", jobId }        // video: the pane shows progress, finishes async
  | { status: "needs_subscription", offer }   // lane 1 unavailable → the app renders the promo card
  | { status: "blocked", reason }
```

A **lane router** picks the lane: the model selected in the session's picker decides the first lane; otherwise the agent asks in the session (decision 2).
Every lane returns the same artifact. The output opens in the artifact-session pane as an image or
video viewer, lands in Outputs, and is saved as an artifact record.

### Lane 1 — Allternit cloud subscription (managed models)
- cloud-api gains `/v1/media/images` and `/v1/media/videos` for the **platform console
  subscription**, with three upstreams:
  - **OpenAI direct**: `gpt-image-2.5-flare` (the default image model) and `gpt-image-2.5-sunburst`
    (edits / reference images).
  - **Higgsfield direct**: Soul / Soul Cinema images, DoP and its video catalog (the
    open-connector runtime shows the request shapes).
  - **OpenRouter**: everything else (Seedream, FLUX 2, Gemini image, Veo, Seedance, Sora, Wan, Kling).
  All three are metered against the plan's media allowance. The console's plan page and model list
  show these media models.
- **Entitlement check** before any call. With no media-enabled plan, the tool returns
  `needs_subscription`, and the transcript shows a **promo / preview card**: sample outputs per model
  family (stills + short looping clips), what the plan includes, and "Get Allternit Cloud"
  (Stripe checkout via the existing `billing_checkout`). The same card appears on the Image and Video
  tiles and in the empty state before the first send.
- The local API's BYOK media plane stays as a lane for users with their own keys (the same card
  shows "or use your own key").

### Lane 2 — Connected subscriptions (Subscription Fabric)
- The router asks the fabric's capability registry for `image.create` / `video.create` (and, for
  other artifact modes, `website.create`, `presentation.create`, …) on the user's connected subs:
  ChatGPT → images, Kimi → websites/docs/slides, etc.
- It is used when a matching sub is connected, as a primary or a fallback per decision 2. This lane
  only calls the fabric's routing contract; it does not rebuild the gateway (Kimi's program owns that).

### Lane 3 — Native: any model, in our harness, by writing code (required)
A harness **plugin + skills** (`allternit-render-kit`) that distills what Claude and other frontier
models do when asked to "make an image/video with code":
- **Image:** the model writes a self-contained SVG, or HTML/CSS/canvas with a fixed viewport. The
  plugin renders it in headless Chrome to PNG (or keeps the SVG) at the requested size, takes a
  still, and runs a check-and-fix pass (the model looks at the render and corrects it; up to 2 rounds).
  Good for: illustrations, diagrams, posters, social cards, UI mockups, charts, logos, vector art.
- **Video:** motion-reel's method, turned into a plugin. The model writes a deterministic
  `draw(t)` canvas scene list on a beat grid. The plugin renders frames in parallel with headless
  Chrome, applies ffmpeg motion blur, and outputs MP4. An optional soundtrack comes from the numpy
  synth, and optional narration from the local Kokoro TTS (audio-gen). Stills contact-sheet review
  comes before the full render. Good for: promos, title cards, explainers, kinetic type, data animation.
- Honest limits, shown in the lane picker: no photoreal people or scenes. That is lanes 1 and 2.
- Renders inside the user's own app (Chromium), with no extra install; see §5. Works with **any** model through gizzi. It costs only tokens, and
  opus 5.5 and astra are the default routed models for it.
- Ships as skills in the harness's skill format, so bots and threads use the same workflow.

## 3. Phases

| Phase | Delivers | Depends on |
|---|---|---|
| **P1** | `media_generate` tool + lane router + **lane 3 image** (in-app render bridge, §5) + output in the pane/Outputs; Image mode contract points at the real tool; no more MODE_EXECUTION_INVALID for Image | — |
| **P2** | **Lane 3 video** (render-kit video: scenes, sub-frame blur, OfflineAudioContext score, WebCodecs + MP4 muxer; render worker for unattended bots); Video contract points at the real tool | P1 |
| **P3** | **Lane 1** (per-tier media cap on the shared allowance): cloud-api media routes (OpenAI gpt-image-2.5 direct, Higgsfield direct, OpenRouter images + videos), platform console plan/model list shows them, entitlement check, `needs_subscription` → **promo card** + tiles/empty-state preview; BYOK lane wired into the router | decisions 1, 3 |
| **P4** | **Lane 2**: router ↔ Subscription Fabric (`image.create` via ChatGPT sub; Kimi → Build/Docs/Slides modes as a backup lane) | Fabric P3 gate |
| **P5** | Same tool for Design canvas (place generated images on the canvas), Build (assets), Swarm (real `agent-swarm` tool name in its contract) | P1 |

Each phase is one PR pair (platform + ai), reviewed by Eoj before merge.

## 4. Decisions (Eoj, 2026-09-27)

1. **Cost:** the subscription's existing allowance covers **all** model inference, media included,
   even when a media model costs more. Each plan tier gets a **per-account cap on media spend** so
   Allternit's cost stays bounded and the plan keeps its margin. At the cap, the tool returns
   `needs_subscription` with an upgrade offer on the same promo card.
2. **Lane choice comes from the model picker.** If the model selected in the session's picker can do
   the job (a media model, or a subscription-backed model), its lane runs first. Otherwise, or when
   that lane fails, the agent **asks in the session** which way to do it next (the card lists the
   available lanes). There is no silent fixed fallback order.
3. **OpenAI (gpt-image-2.5) and Higgsfield:** both. Allternit-funded under the subscription, and a
   user's own key takes precedence when added.
4. **Native rendering must ship with the product, not rely on a developer Mac.** See §5.

## 5. Native lane in production: no extra dependencies

Every Allternit surface already contains a Chromium engine: Desktop is Electron, the web app runs in
the user's browser, and cloud computers have a browser. Chromium can do the whole native pipeline
without Python, ffmpeg or a separate Chrome install:

| Step | Dev prototype (motion-reel) | Production (render-kit) |
|---|---|---|
| Frames | headless Chrome + Playwright | the app's own Chromium, in a sandboxed offscreen iframe / Electron offscreen window |
| Motion blur | ffmpeg `tmix` | sub-frame accumulation on canvas (render N sub-frames, blend) |
| Soundtrack | numpy synth → WAV | `OfflineAudioContext` (deterministic, faster than real time) |
| Encode | ffmpeg → H.264 MP4 | **WebCodecs** `VideoEncoder` (H.264) + `AudioEncoder` (AAC/Opus) + a JS MP4 muxer (e.g. `mediabunny`) |
| Image | Playwright screenshot | canvas / SVG rasterize → PNG (`OffscreenCanvas.convertToBlob`) |

**Where it runs:** the same bridge as `pane_artifact`. gizzi's `media_generate` (native lane) publishes
a render request; the app showing the session renders it and uploads the PNG/MP4 as the artifact.
When no app is attached (a bot or thread running unattended), the job goes to a **render worker**:
headless Chromium in the cloud computer golden image (add it there), or a small cloud render service.
Either way it runs the same render-kit page, so the output is identical everywhere.

WebCodecs is available in Chromium/Electron and Safari 17+ (iOS included). The one addition to the
bundle is the muxer (a JS library, tens of KB).
