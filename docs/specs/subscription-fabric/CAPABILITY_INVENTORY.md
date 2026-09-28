# Subscription Fabric — Capability Inventory (ChatGPT · Claude · Kimi)

Researched 2026-09-28 from each provider's own help center / pricing / release
notes (third-party sources marked). Purpose (owner, 2026-09-28): the fabric is
"a way to use your subscription as a model" — every feature a web subscription
offers becomes a gateway capability, surfaced in Allternit's model selector
(chat) and as tools/artifacts, generated on the Sessions machine (D15) and never
on the user's own computer. Re-verify against the live UI before building each
adapter (P3 lesson: probe first, on a copy profile, headed).

## 0. Decision needed before building beyond P3 — provider terms

All three research passes flagged the same risk. OpenAI's help center quotes its
Terms prohibiting "automatically or programmatically extracting data", "making
your account available to anyone else", and "using ChatGPT to power third-party
services" ([Pro tiers](https://help.openai.com/en/articles/9793128),
[Business limits](https://help.openai.com/en/articles/12003714-chatgpt-business-models-limits)).
Anthropic's and Moonshot's consumer terms were not checked in detail (flagged
unverified). Enforcement noted by OpenAI: temporary usage restrictions. This is
not legal advice; it is an explicit owner go/no-go (and applies to the existing
`chatgpt-image` lane too).

## 1. Model-selector entries (chat → `subs/<provider>:<model_class>`)

Keep the four router classes; add finer model ids + an `effort` parameter,
because quotas and silent fallbacks are per model (the router must see which
model actually answered). Scrape the live picker per account — plans differ.

| Class | ChatGPT (Plus / Pro) | Claude (Pro / Max) | Kimi (paid) |
|---|---|---|---|
| `fast` | Instant (GPT-5.6 Sol; Luna on Free/Go) | Sonnet 5.5 · low effort (Haiku not confirmed in picker) | K2.6 · Standard (free, no credits) |
| `standard` | Medium | Sonnet 5.5 (default Medium) | K3 · Standard |
| `reasoning` | High / Extra High | Opus 5.5 | K3 · Advanced (Extreme optional) |
| `deep` | Pro (GPT-6 Pro / 5.6 Sol Pro; hard weekly quotas, silent fallback to Medium) | Fable 5.1 (Pro: credits only; Max: ≤50% weekly) | K3 Swarm · Extreme (agent task, not streaming) |
| extra | Work models under `subs/chatgpt-work:<model>` (separate allowance) | effort low→max per model | K3 1M context (Allegro+) |

## 2. Capability matrix

Legend: ✅ native · 🧩 via code/agent (indirect) · — none · 🔒 human-approval gate
(external side effect / money / unattended). "Surface" = where it lands in
Allternit (HARDENING §M / P6): **picker** (chat mode model), **thread** (progress
feed + artifact card in chat/cowork), **artifact** (artifact store → preview /
Files), **approval** (cowork approval card), **MCP** (tool for gizzi-code / bots).

| Capability id | ChatGPT | Claude | Kimi | Output types | Surface |
|---|---|---|---|---|---|
| `chat.create` / `chat.continue` | ✅ (P3 live) | ✅ | ✅ | text | picker, thread |
| `research.run` quick (web search) | ✅ | ✅ | ✅ (chat tools) | text + citations | thread, MCP |
| `research.run` deep / `research.continue` / `research.clarify` | ✅ plan-approve step; MD/Word/PDF | ✅ Research (paid) | ✅ 10–25 min, clarify step; PDF/Word + HTML visual report | document, pdf, html_app | thread (long-running), artifact |
| `image.generate` | ✅ Images 2.5 (P3 live, Allternit project) | 🧩 by code (SVG / canvas / PNG) | ✅ Kimi Design 1K–4K, transparent bg | image | thread, artifact, media-router |
| `image.edit` | ✅ region editor | 🧩 code (PIL) — verify | ✅ Design refine | image | thread, artifact |
| `video.generate` | — (Sora discontinued 2026-04-26) | 🧩 HTML/canvas animation; MP4 rendered by Allternit | ✅ agent tool, 4–12 s | video, html_app | artifact, media-router |
| `audio.tts` / `audio.sfx` | — (voice is live-only; exclude) | 🧩 Web Audio in artifact (verify) | ✅ TTS (Mandarin voices only) / SFX 0.5–22 s | audio | artifact |
| `document.create/edit/export` | ✅ Documents plugin / Work → DOCX, Google Doc | ✅ Claude Docs (beta) + file creation → DOCX/PDF/MD | ✅ Docs → DOCX (tracked changes), PDF, MD | document, pdf | thread, artifact (office preview) |
| `presentation.create/edit/export` | ✅ Presentations plugin → PPTX | ✅ Claude Slides (beta) → PPTX/PDF | ✅ Slides → editable PPTX, 3–10 min provider-side | presentation | thread, artifact (office-pptx preview) |
| `spreadsheet.create/edit/analyze/export` | ✅ Spreadsheets plugin (XLSX/CSV/TSV) + data analysis | ✅ file creation → XLSX w/ formulas; analysis tool | ✅ Sheets → XLSX | spreadsheet, image (charts) | thread, artifact |
| `pdf.create` / `pdf.extract` | ✅ PDF plugin | ✅ file creation | ✅ Docs → PDF | pdf | artifact |
| `website.create/modify/preview` | ✅ Sites (beta, Work) | ✅ Artifacts (HTML/React), Claude Design | ✅ Kimi Build: live preview, versions, DB | website, html_app, code_project | thread, artifact (sandboxed preview) |
| `website.export` | — (not documented) | ✅ artifact download / Design ZIP | ✅ full project ZIP | archive, code_project | artifact |
| `website.publish` / `.unpublish` 🔒 | 🔒 Sites deploy (every URL is production) | 🔒 publish/embed artifact (public on Pro/Max; unpublish deletes data) | 🔒 `{name}.ok.kimi.link` (+ DB, logins) | url | **approval** (D9) |
| `design.create/export` / `design.handoff` 🔒 | — | ✅ Claude Design → ZIP/PDF/PPTX/HTML; 🔒 hand-off to Canva/Vercel/… | ✅ (Design = images) | website, presentation, archive | artifact, approval |
| `agent.task.run/continue/approve` | ✅ ChatGPT Work (cloud, plan mode, approvals) | ✅ unified agentic tasks (ex-Cowork, keeps running) | ✅ Agent (ex-OK Computer, 5–20 min) | mixed | thread (detached, D11/D12), approval |
| `agent.swarm` | — | — | ✅ K3 Swarm (paid; expensive — cost gate) | mixed | thread, approval (cost) |
| `code.task.run` 🔒 | — (Codex not on web) | 🔒 Claude Code on the web → branch + **PR** | — (Kimi Code is CLI) | code_project | approval |
| `project.create/open/list/attach_file/continue` | ✅ Projects (P3 uses one) | ✅ Projects + knowledge | ✅ Projects (20–100) | — | thread (context) |
| `file.upload` / `file.analyze` | ✅ 512 MB/file | ✅ 20 files/chat | ✅ 100 MB, 50 files | — | thread input |
| `file.download` / `library.*` | ✅ Library (primary retrieval of generated files) | ✅ per-artifact download | ✅ per-card download | any | artifact |
| `template.create/list` | ✅ Template Creator | ✅ Skills (`skill.*`, account state 🔒-light) | — | — | settings |
| `task.schedule` / `task.trigger` 🔒 | 🔒 scheduled + event-triggered tasks | 🔒 scheduled agentic tasks | 🔒 scheduled tasks | — | approval |
| `connector.invoke` / `plugin.invoke` 🔒 | 🔒 1,400+ apps (Gmail send, Slack, GitHub…) | 🔒 Gmail/Calendar/Drive/M365/MCP writes | 🔒 plugins | — | approval |
| `memory.*` | read-only; writes 🔒-light | read-only; import/edit 🔒-light | read-only | — | settings |
| `study.*` | ✅ flashcards/quizzes (in-chat only) | — | — | html_app | thread |

## 3. Excluded (not automatable on web, device-bound, or retired)

- **ChatGPT:** voice/video/screen share, Sketch (canvas input), Codex & Work Local & Atlas (retired), Office add-ins, Chrome extension, Health/Finances, Sora, Canvas (legacy, sunsetting), custom GPT creation (all GPTs retire 2026-12-11), Instant Checkout / reservations / credit purchases (money — never automated).
- **Claude:** Claude Science, computer use, desktop tasks, Claude in Chrome, Office add-ins, voice mode, Mythos (not on consumer plans), incognito for file work, buying usage credits.
- **Kimi:** Kimi Work (desktop, controls local computer), Kimi Code (CLI), Claw's WeChat/Feishu/DingTalk links, mainland kimi.com (+86 phone). Target the international site **kimi.ai** (the kimi.com/kimi.ai split caused auth bugs elsewhere).

## 4. Cross-cutting design consequences

1. **Most deliverables are long-running, provider-side tasks** (Kimi Agent 5–25 min, ChatGPT Work, Claude agentic tasks, deep research everywhere). The P3 contract already has detached execution (D11 progress + D12 push); P5 must prove it end-to-end (submit → leave → artifact delivered without polling).
2. **Clarify / plan-approval steps** (deep research on all three, ChatGPT Work plan mode): a new `needs_user`-style interaction the caller answers (or a policy "include everything"), surfaced as an approval card.
3. **Quotas are pooled and opaque** (Claude 5 h + weekly pool; Kimi monthly credits; ChatGPT per-feature allowances with silent fallback). Pools must read the provider's own counters/banners, and expensive capabilities (Swarm, deep research, Pro models) need a cost-class gate like the media-router's.
4. **Retrieval differs per provider:** ChatGPT → Library, Claude → artifact panel/download, Kimi → result card / project ZIP. One `file.download` capability, three adapter implementations.
5. **Every 🔒 row routes through the D9 approval gate** — no policy override.

## 5. Build order (P5–P6 as revised with the owner, 2026-09-28)

1. **Owner decision on §0.**
2. **Sessions machine = the permanent home** (done 2026-09-28: Desktop-registered `sessions` computer, reached via the Desktop API proxy).
3. **P5a claude-web:** chat (+ model/effort), artifacts → `website.*`/`document.*`/`presentation.*` (Docs/Slides/Design), file creation, research. Owner logs in once via the Sessions machine's streamed display.
4. **P5b kimi-web (kimi.ai):** chat, Agent → slides/docs/sheets/websites (detached), deep research (clarify step), Design images, video.
5. **P5c chatgpt-web expansion:** Work plugins (documents/presentations/spreadsheets/PDF), deep research, Sites (gated), Library retrieval.
6. **P5d MCP surface** in the gateway (tools for gizzi-code / bots).
7. **P6 platform surfaces:** wire the D13 `subs/*` catalog into the allternit-api picker (`provider_routes.rs`) and gizzi-code, in-thread progress + artifact cards, approval cards for 🔒, Desktop Sessions panel (status, accounts, needs-user queue, streamed-display login).

Sources: full per-provider research notes with citations were delivered in the
2026-09-28 session (claude session, HANDOFF §15); key first-party pages —
ChatGPT: help.openai.com (release notes 6825453, models 11909943, Work 20001275,
Sites 20001339, Library 20001052, images 11084440, Sora 20001152), chatgpt.com/pricing ·
Claude: support.claude.com (release notes 12138966, models 8664678, files 12111783,
artifacts 17153992, Docs 16923645, Design 14604416, Research 11088861, usage
11647753, Claude Code web 12618689), claude.com/pricing · Kimi: kimi.ai/help
(membership-pricing, model-mode-selection, docs-and-sheets, websites,
deep-research, others/capability), kimi.com/en/help (agent, agent-swarm, ppt).
