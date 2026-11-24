# Allternit Meetings — rules for every build lane (read before you start)

**Sources of truth, in order:**
1. `docs/meetings/CONTRACT.md` in the platform worktree. It is frozen. Shapes, routes, events, MCP tools and states come from here. If you need a change, stop and report it; do not invent one.
2. The plan: `/Users/joe/.claude/plans/nifty-hugging-falcon.md`. Read Seamless UX, Architecture, Surface Matrix and your step.
3. Spike facts: `/Users/joe/Desktop/allternit-workspace/SPEC-meetings-2026-11-12.md` and `services/meeting-bot/spike/` (README, join.ts, meet-selectors.ts, gizzi-media.ts, peer-capture.ts). The spike proved these: a signed-in bot Google account gets admitted to Meet; the camera tile is the `/meeting-tile` page CDP-screencast into a canvas; remote audio is captured by wrapping RTCPeerConnection.
4. Design, picked by Eoj on 11-13. Page: `/private/tmp/claude-501/-Users-joe/cf04b42c-f228-4703-a12d-772cededc721/scratchpad/meetings-design.md`. Real screenshots: the `real/` folder next to it.
   - Meetings list: an agenda list. The live call sits on top, each meeting has a Gizzi switch, and past calls show "Recap".
   - Live page: the live voice view look. Mascot stage, side panel with Transcript / Artifacts / Approvals tabs, and Stop listening / Speak replies / Leave controls.
   - Connect step: "Connect Google Calendar" or "Paste a calendar link" (Outlook, iCloud). After connecting, the auto-join rule is picked inline. The default is all except 1:1s.
   - Pasted call link in chat: the MCP app card from `allternit-ai-meetings/src/lib/app-screens/packs/meetings.json`.
   - States everywhere: Joining / Waiting to be let in / Live / Not let in / Recap ready.
   - New UI uses white and `--neutral-fill`, never tan surfaces. Dark mode is graphite.

**Worktrees** (both on branch `feat/meetings`; do not create new ones):
- Platform: `/Users/joe/Desktop/allternit-workspace/allternit-meetings`. Contains cloud-api, allternit-api, services/voice, services/meeting-bot, gizzi-code, iOS, docs.
- Web, Desktop UI and PWA: `/Users/joe/Desktop/allternit-workspace/allternit-ai-meetings`.

Other lanes edit the same worktrees at the same time. Rules for sharing:
- Touch only the files your brief names, plus the one or two registration lines you need, such as a `mod` line or a route mount.
- Never reformat or reorder files you don't own.
- Never run `git checkout` / `git stash` / `git reset`.
- Never commit or push.

**Surface Matrix:**

| Surface | Status |
|---|---|
| Desktop | BUILD |
| Web + phone layout | BUILD |
| PWA | BUILD |
| iOS | BUILD |
| Android | HAS (via PWA) |
| Gizzi Code | BUILD |
| Backend | BUILD |
| Docs | BUILD |

**House rules:**
- **Package managers:** pnpm only in these worktrees. The spike folder is npm. Add a dependency only if your step needs it, and say so in your report.
- **Rust:** `CARGO_TARGET_DIR=/Users/joe/Desktop/allternit-workspace/.shared-target/allternit-meetings`. Never set RUSTC_WRAPPER or CARGO_BUILD_JOBS. Never `pkill` cargo or rustc. Prefer `cargo check` while iterating. To wait for a free cargo: `until [ -z "$(pgrep -x cargo)" ]; do sleep 15; done`.
- **Typecheck web code** with `npx tsrs --project tsconfig.typecheck.json --noEmit` in allternit-ai-meetings.
- **Tests:** only for tricky logic or a real bug. Assert what the user gets. Never mock our own code. No snapshot tests, no skipped tests.
- **Servers:** stop every dev server, browser and simulator you start. Never deploy, publish, or run a release or build-queue job.
- **Accounts:** never touch Google sign-in pages or Eoj's Safari. Never print secrets. Keychain items: `allternit-zoom-app-dev`, `allternit-google-calendar-oauth`.
- **Libraries.dev:** for React UI, read `/Users/joe/.agents/skills/libraries-dev/SKILL.md` first. For motion, read `/Users/joe/.agents/skills/ui-motion/SKILL.md`.
- **Fix it now:** if you find a bug or gap inside your lane, fix it. If it belongs to another lane, report it with `file:line`.
- **Budget:** about 150 tool calls. If you can't finish, stop at a working checkpoint and report exactly what is left.

**Report format:**
1. Files changed.
2. Done-check output, pasted.
3. Screenshots (paths) for UI work.
4. Contract questions.
5. What is left, if anything.
