# HANDOFF — Media lanes + artifact-session agent reach (2026-09-28)

Session joe-58 (Claude) handed this off because its context filled up. Read `PLAN.md` in this folder first.

## Done and merged
- **platform #826 + ai #156:** the artifact-session agent edits the open Docs/Sheets/Slides document.
  gizzi `pane_artifact` tool (describe/read/call) → the app runs Allternit Office's own skills against
  the live editor (`registerPaneAgent` / `registerPaneAgentPersist` in the office-suite). The Docs skill
  was ported from GenOffice. Agent edits save immediately. Desktop b3979 (Agent B, joe-a2) includes both.
  - **Live test with a model: NOT DONE.** Eoj said "later"; someone was using the Desktop main window.
    Test: Artifacts → Document, send "Describe this document, then add a heading 'Test' and one
    sentence under it". Watch over CDP :9222 read-only unless Eoj allows driving it.
- **platform #829:** `PLAN.md` (three lanes; Eoj's decisions in §4; the native lane runs in the app's
  Chromium, §5).

## Open (awaiting CI, then Eoj's OK to merge; merge platform first)
- **platform #857** `session/media-p1-native` (worktree `allternit-wt-media-p1`): gizzi
  `media_generate` tool + `PaneRender` bridge + routes, allternit-api/cloud-api relays, tests.
  CI Typecheck failed on the tool's return type (metadata shapes differed); fixed in 741348e7a, and
  CI is re-running. Local gizzi `tsc` didn't show the error, so trust CI; locally symlink
  `cmd/gizzi-code/node_modules` → the main checkout's, and remove it before committing.
- **ai #175** `session/media-p1-native` (worktree `allternit-ai-wt-office`): `lib/media/native-render.ts`
  (sandboxed iframe, no-network CSP, inline canvas script, HTML via foreignObject; checked in Chrome
  under the prod CSP incl. escape/fetch attempts), `pane-render-bridge`, `usePaneRender` in Cowork +
  Chat, media_generate → Outputs (`artifact-from-part.ts`), Image/Video contracts →
  `media_generate`, Image-session auto-open. Full vitest (4059) + build green; CI green.
- After both merge: ask Agent B (joe-a2) for a Desktop build, then live-test Image mode ("make a
  poster for …") and the Docs session test above.

## Next phases (PLAN.md §3)
- **P2 native video:** render-kit video in the app: deterministic `draw(t)` scenes, sub-frame motion
  blur, OfflineAudioContext score, WebCodecs VideoEncoder/AudioEncoder + JS MP4 muxer (e.g.
  mediabunny); a render worker (headless Chromium on the cloud computer image) for unattended bots;
  Safari → worker until verified. `media_generate` kind "video" currently answers "not available".
  Template: the `~/.claude/skills/motion-reel` method.
- **P3 cloud lane:** cloud-api `/v1/media/{images,videos}`: OpenAI direct (`gpt-image-2.5-flare` default,
  `-sunburst` for edits), Higgsfield direct (`platform.higgsfield.ai`, `Authorization: Key key:secret`),
  OpenRouter `/api/v1/images` + `/api/v1/videos` for the rest. Metered from the plan's normal
  allowance with a per-tier media spend cap; `needs_subscription` → promo card + upsell. BYOK keys
  take precedence. The model picker's model decides the first lane; otherwise ask in the session.
  **Keys are already on the server** (`/opt/allternit-cloud-api/.env` on `mail`: `OPENAI_API_KEY`,
  `HIGGSFIELD_API_KEY`, `HIGGSFIELD_API_SECRET`; see Brain `Infra/cloud-provider-keys.md`); the
  service wasn't restarted.
- **P4:** Subscription Fabric lane (Kimi's program owns the gateway). **P5:** Design / Build / Swarm.
- Also open: Desktop never passes `ALLTERNIT_INTERNAL_SERVICE_TOKEN` to gizzi, so the allternit-tools
  MCP (`document_to_markdown`) doesn't load in Desktop.

## Eoj's rules for this work
Describe each PR and get his OK before merging; one owner per item (owner table in allternit-ai
`docs/PARITY_APPROVED_NOTES_AUDIT.md`, row "Artifact-session agent reach"); ship only via Agent B's
Desktop builds; never drive the Desktop main window while he's using it.
