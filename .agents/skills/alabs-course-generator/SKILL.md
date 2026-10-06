---
name: alabs-course-generator
description: Fork an external course catalog at full section depth, then generate Allternit-branded A://Labs HTML modules with production-grade interactive media replacing lecture videos.
---

# A://Labs Course Generator

Generate a new A://Labs course by deeply forking an external course catalog (e.g., Anthropic Academy) and rewriting it for the Allternit platform. The output must match the depth of the existing starter modules and replace vendor lecture videos with real, open-source interactive media.

## When to use this skill

- A stakeholder wants to mirror an external course catalog inside A://Labs.
- The external catalog has detailed sections/lessons that must be preserved, not flattened into shallow overviews.
- You need consistent Allternit branding, source-grounded examples, and video replacements that are not cheesy canvas hacks.

## Output barometer

Every generated module must be as thorough as:

- `alabs-generated-courses/ALABS-AGENTS-API-module1.html`
- `alabs-generated-courses/ALABS-AGENTS-API-module2.html`
- `alabs-generated-courses/ALABS-AGENTS-API-module3.html`
- `alabs-generated-courses/ALABS-AGENTS-API-module4.html`
- `alabs-generated-courses/ALABS-AGENTS-API-module5.html`
- `alabs-generated-courses/ALABS-AGENTS-API-module6.html`
- `alabs-generated-courses/ALABS-AGENTS-API-module7.html`
- `alabs-generated-courses/ALABS-AGENTS-API-module8.html`
- `alabs-generated-courses/ALABS-AGENTS-API-module9.html`
- `alabs-generated-courses/ALABS-AGENTS-API-module10.html`
- `alabs-generated-courses/ALABS-OPS-COWORK-module1.html`
- `alabs-generated-courses/ALABS-OPS-COWORK-module2.html`
- `alabs-generated-courses/ALABS-OPS-COWORK-module3.html`
- `alabs-generated-courses/ALABS-OPS-COWORK-module4.html`
- `alabs-generated-courses/ALABS-OPS-COWORK-module5.html`
- `alabs-generated-courses/ALABS-CORE-FLUENCY-module1.html`
- `alabs-generated-courses/ALABS-CORE-FLUENCY-module2.html`
- `alabs-generated-courses/ALABS-CORE-FLUENCY-module3.html`
- `alabs-generated-courses/ALABS-CORE-FLUENCY-module4.html`
- `alabs-generated-courses/ALABS-CORE-FLUENCY-module5.html`
- `alabs-generated-courses/ALABS-CORE-FLUENCY-module6.html`

Hard requirements:

- **Allternit-branded** — zero mentions of Claude, Anthropic, or other vendor brands in learner-facing HTML.
- **Source-grounded** — every code block and capstone references real Allternit packages/surfaces.
- **Section-depth preserved** — if the source course has 7 sections with 67 lessons, the mapped Allternit course should have a comparable number of modules and each module should cover equivalent lesson-level detail.
- **Interactive** — progress bar, fixed nav, scroll-reveal, code-copy buttons, 3 quizzes with instant feedback + retry, and a capstone.
- **Rich media** — at least one production-grade video replacement per module (Asciinema terminal recording, Code-Hike-style step-through, Mermaid diagram, or Manim animation).
- **Buildable** — `npx tsx alabs-module-template/scripts/build.ts` succeeds with zero unresolved `{{` placeholders.

## Workflow

### Phase 1: Deep scrape

1. Identify the external collection/course (e.g., `https://academy.claude.com/collections/build-with-claude`).
2. Scrape every linked course page with `curl` + Python. Do not rely only on JS-rendered SPA content.
3. Persist three artifacts in the session worktree:
   - `{source}-academy-raw.json` — full page HTML/text
   - `{source}-academy-summary.json` — compact catalog
   - `{source}-academy-analysis.json` — parsed structure:
     - title, description, counts (lessons/quizzes/hours)
     - learning objectives
     - audience & prerequisites
     - `sections`: each with `title`, `lessons`, `description`, and `lessonTitles`

Use the Anthropic analysis as the parser target: `docs/research/data/anthropic-academy-analysis.json`.

### Phase 2: Deep mapping

1. Create `{SOURCE}_TO_ALABS_MAPPING.md` modeled on `docs/learnings/ANTHROPIC_TO_ALABS_MAPPING.md`.
2. For **every external section**, propose an Allternit module or submodule.
3. Map external lesson titles to specific Allternit concepts, source files, and capstone tasks.
4. Annotate which open-source media tool will replace videos for each section (see Media Stack below).

### Phase 3: Outlines

Write `alabs-generated-courses/content/course-outlines.json` for the starter courses, including:

- `courseCode`, `title`, `tier`, `description`, `estimatedHours`
- `sourcePackages`
- `modules` array with `position`, `title`, `focus`, `mediaType`, and `sourceFiles`

### Phase 4: Module content JSON

For each module:

1. Read every source file listed in the outline.
2. Write `alabs-generated-courses/content/ALABS-{TIER}-{COURSE}-module{N}.json`. See `ALABS-AGENTS-API-module2.json`, `ALABS-AGENTS-API-module4.json`, `ALABS-AGENTS-API-module5.json`, `ALABS-AGENTS-API-module9.json`, `ALABS-OPS-COWORK-module2.json`, `ALABS-OPS-COWORK-module4.json`, and `ALABS-CORE-FLUENCY-module2.json` for current barometers.
3. Add a `mediaAssets` array referencing source files in `alabs-generated-courses/media/src/`:
   ```json
   {
     "mediaAssets": [
       { "id": "adapter-flow", "type": "mermaid", "src": "alabs-generated-courses/media/src/adapter-flow.mmd" },
       { "id": "cowork-setup", "type": "asciinema", "src": "alabs-generated-courses/media/src/cowork-setup.cast" }
     ]
   }
   ```
4. The `moduleContent` must include:
   - Hero with badge, title, subtitle, meta, source package
   - Multiple deep-dive sections matching the external section/lesson detail
   - Code blocks grounded in real Allternit source
   - At least one media placeholder from the Media Stack: `{{MEDIA:adapter-flow}}`
   - 3 interactive quizzes using the shared quiz engine
   - Capstone project

### Phase 5: Build and audit

```bash
cd ~/Desktop/allternit-workspace/allternit-session-5e9f20da-b157-41e1-a340-91ec19bdef56
npx tsx alabs-module-template/scripts/build.ts \
  --content alabs-generated-courses/content/ALABS-{TIER}-{COURSE}-module{N}.json \
  --output alabs-generated-courses/ALABS-{TIER}-{COURSE}-module{N}.html
```

Recent examples:
- `ALABS-AGENTS-API-module2.json` → `ALABS-AGENTS-API-module2.html`
- `ALABS-AGENTS-API-module3.json` → `ALABS-AGENTS-API-module3.html`
- `ALABS-AGENTS-API-module4.json` → `ALABS-AGENTS-API-module4.html`
- `ALABS-AGENTS-API-module5.json` → `ALABS-AGENTS-API-module5.html`
- `ALABS-AGENTS-API-module6.json` → `ALABS-AGENTS-API-module6.html`
- `ALABS-AGENTS-API-module7.json` → `ALABS-AGENTS-API-module7.html`
- `ALABS-AGENTS-API-module8.json` → `ALABS-AGENTS-API-module8.html`
- `ALABS-AGENTS-API-module9.json` → `ALABS-AGENTS-API-module9.html`
- `ALABS-AGENTS-API-module10.json` → `ALABS-AGENTS-API-module10.html`
- `ALABS-OPS-COWORK-module2.json` → `ALABS-OPS-COWORK-module2.html`
- `ALABS-OPS-COWORK-module3.json` → `ALABS-OPS-COWORK-module3.html`
- `ALABS-OPS-COWORK-module4.json` → `ALABS-OPS-COWORK-module4.html`
- `ALABS-OPS-COWORK-module5.json` → `ALABS-OPS-COWORK-module5.html`
- `ALABS-CORE-FLUENCY-module2.json` → `ALABS-CORE-FLUENCY-module2.html`
- `ALABS-CORE-FLUENCY-module3.json` → `ALABS-CORE-FLUENCY-module3.html`
- `ALABS-CORE-FLUENCY-module4.json` → `ALABS-CORE-FLUENCY-module4.html`
- `ALABS-CORE-FLUENCY-module5.json` → `ALABS-CORE-FLUENCY-module5.html`
- `ALABS-CORE-FLUENCY-module6.json` → `ALABS-CORE-FLUENCY-module6.html`

Audit every output:

```bash
for f in alabs-generated-courses/ALABS-*.html; do
  echo "$f"
  echo "  placeholders: $(grep -c '{{' "$f" || true)"
  echo "  vendor mentions: $(grep -icE 'claude|anthropic|academy\.claude|skilljar' "$f" || true)"
  echo "  size: $(wc -c < "$f") bytes"
done
```

Target: placeholders = 0, vendor mentions = 0, size > 20 KB.

### Phase 6: Sync (when Canvas course IDs exist)

1. `scripts/create-advanced-courses-browser.ts` for course creation.
2. `scripts/sync-incremental.ts` for modules.
3. `scripts/canvas-quiz-sync.ts` for scored quizzes.
4. `scripts/fix-unpublished-modules.ts` for prerequisites.
5. `scripts/launch-audit.ts` for validation.

## Media pipeline (implemented)

The `alabs-module-template` now has a built-in media pipeline. Do not hand-roll cheesy canvas loops as the primary video replacement.

### How it works

1. Store source media files in `alabs-generated-courses/media/src/`:
   - `.mmd` for Mermaid diagrams
   - `.cast` for Asciinema terminal recordings
   - `.json` for Code-Hike-style walkthroughs (Phase 2)
   - demo folders for Sandpack (Phase 3)
   - `.py` for Manim and `.tsx` for Remotion (Phase 4)
2. Reference them in the module content JSON `mediaAssets` array.
3. Embed them in `moduleContent` with `{{MEDIA:asset-id}}`.
4. Run `build.ts`. It will:
   - Call the appropriate per-media builder in `alabs-module-template/scripts/media/`.
   - Inline small assets (Mermaid source, Asciinema cast data) into the HTML.
   - Inject the necessary runtime CSS/JS (Mermaid, Asciinema player) into `<head>`.
   - Leave larger outputs (Manim/Remotion MP4s, Sandpack demos) in `media/dist/` or `demos/` and link/embed them.

### Phase 1: Inline media (ready now)

- **Mermaid**: `alabs-module-template/scripts/media/build-mermaid.ts`
- **Asciinema**: `alabs-module-template/scripts/media/build-asciinema.ts`

Example module using both: `alabs-generated-courses/content/ALABS-AGENTS-API-module1.json`.

### Phase 2: Code-Hike-style walkthroughs (ready now)

- **Walkthrough**: `alabs-module-template/scripts/media/build-walkthrough.ts`
- Source format: JSON pointing to a code file + an array of steps with highlighted lines and notes.
- Output: static HTML with step navigation, line highlighting, and dimmed context lines.
- This is a self-contained replacement for Code Hike that does not require React/MDX in the module runtime.

### Phase 3-4: Coming next

- Sandpack demo bundling for runnable playgrounds
- Manim/Remotion video rendering for high-concept animations

## Media stack: open-source video replacements

Do not use cheesy canvas loops as the primary video replacement. Use these tools instead, depending on the concept:

### 1. Asciinema — terminal workflows

- **What**: Records terminal sessions as text-based `.cast` files and plays them back in the browser.
- **Why**: Perfect for Claude Code-style demos, CLI usage, agent orchestration, and Cowork task loops. Searchable, copy-pasteable, tiny file size.
- **License**: GPL v3 (recorder), Apache 2.0 (server).
- **Self-host**: `asciinema/asciinema-server` can be self-hosted.
- **Embed**: `<asciinema-player src="/labs/casts/demo.cast" theme="monokai"></asciinema-player>`
- **Use when**: demonstrating terminal commands, agent runs, git workflows, code generation sessions.

### 2. Code-Hike-style walkthroughs

- **What**: Self-contained step-through of a source file. Highlights lines, dims context, and shows an explanation per step.
- **Why**: Replaces "follow along with the instructor" videos with a self-paced, interactive code narrative without pulling React/MDX into the module runtime.
- **Implementation**: `alabs-module-template/scripts/media/build-walkthrough.ts`
- **Source format**: JSON with `file`, `language`, and `steps` (each step has `title`, `lines`, `note`).
- **Embed**: `{{MEDIA:walkthrough-id}}` renders a step navigator + line-highlighted code block.
- **Use when**: walking through a source file, explaining an algorithm, or building a feature line-by-line.

### 3. Mermaid — architecture diagrams

- **What**: Diagrams as code (flowcharts, sequence diagrams, DAGs, state machines).
- **Why**: Replaces whiteboard/explainer videos with precise, version-controlled diagrams.
- **License**: MIT.
- **Embed**: Load `mermaid.min.js` from CDN and render `<pre class="mermaid">...</pre>` blocks inside module HTML.
- **Use when**: agent loop diagrams, provider-adapter flow, Cowork task orchestration, workflow engine DAGs.

### 4. Sandpack — runnable code playgrounds

- **What**: Embeddable browser-based code editor + runner from CodeSandbox.
- **Why**: Replaces "watch me run this code" videos with "run this code yourself".
- **License**: Apache 2.0 (`codesandbox/sandpack`).
- **Integration**: Easiest inside a React/Vite surface; for single HTML modules, embed via iframe from a hosted Sandpack route or use `alabs-generated-courses/demos/` pages.
- **Use when**: learner should experiment with a code snippet (provider adapters, prompt patterns, MCP tools).

### 5. Manim — precise animations

- **What**: Python animation engine for explanatory videos (3Blue1Brown).
- **Why**: Generates broadcast-quality animations for hard-to-explain concepts (token prediction, RAG retrieval, agent routing).
- **License**: MIT (ManimCommunity/manim).
- **Integration**: Render `.mp4` or `.gif` and link/embed from module HTML.
- **Use when**: a concept truly needs motion and diagrams are not enough.

### 6. Remotion — programmatic video

- **What**: React library for generating videos programmatically.
- **Why**: If Allternit eventually wants to produce real lecture videos at scale from components/code.
- **License**: Remotion license (free for <3000 views, paid for higher volume) — evaluate before committing.
- **Use when**: you need actual video files but want them generated from code, not shot in a studio.

## Media selection guide per module type

| Course type | Primary media | Secondary media |
|---|---|---|
| API / provider adapters | Code Hike walkthrough + Mermaid diagram | Asciinema for CLI demos |
| Cowork / automation | Asciinema terminal recordings | Mermaid workflow diagrams |
| Agent architecture | Mermaid diagrams + Manim animations | Code Hike for loop code |
| AI Fluency / concepts | Manim animations + Mermaid 4D framework | Code Hike for prompt examples |
| Plugin SDK / coding | Code Hike + Sandpack runnable demos | Asciinema for build/test CLI |

## Branding guardrail

Before any module is complete, run:

```bash
for f in alabs-generated-courses/ALABS-*.html; do
  echo "$f: $(grep -icE 'claude|anthropic|academy\.claude|skilljar' "$f" || true) vendor mentions"
done
```

Target: **0** vendor mentions in learner-facing HTML. The mapping documents and raw scrape JSONs may still mention the external source for traceability.

## Templates to copy

- Deep mapping: `docs/learnings/ANTHROPIC_TO_ALABS_MAPPING.md`
- Outlines: `alabs-generated-courses/content/course-outlines.json`
- Module content: `alabs-generated-courses/content/ALABS-AGENTS-API-module1.json`
- RAG module: `alabs-generated-courses/content/ALABS-AGENTS-API-module4.json`
- MCP module: `alabs-generated-courses/content/ALABS-AGENTS-API-module5.json`
- Engineering harness module: `alabs-generated-courses/content/ALABS-AGENTS-API-module9.json`
- Engineering graph module: `alabs-generated-courses/content/ALABS-AGENTS-API-module10.json`
- Cowork scaling module: `alabs-generated-courses/content/ALABS-OPS-COWORK-module4.json`
- Fluency capstone module: `alabs-generated-courses/content/ALABS-CORE-FLUENCY-module6.json`
- Build command: Phase 5 above
