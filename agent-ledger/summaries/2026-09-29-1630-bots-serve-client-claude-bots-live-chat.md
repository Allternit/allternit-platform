# /bots terminal chat as a live client of the bot's shared thread

- **Session:** Claude Code (Opus 5.5), 2026-09-29, resumed from `HANDOFF-login-and-bots-2026-09-29.md`
- **PR:** #961, merged as `e31205053` (merge commit). Branch `session/bots-serve-client` and its worktree deleted.

## What was done
- Terminal `/bots` chats are live clients of the bot's platform thread: the local bot is registered on the platform, the history loads, the chat follows `/sync` over SSE, relays permission and question prompts, follows handoffs, and dedupes replies (commits 978927dad..7301d90f0 from earlier sessions).
- **Missing user messages, root cause fixed (this session):**
  - `allternit-api` `transform_bus_event` published each `message.updated` as the session's *newest* message. A turn's reply row lands about 3 ms after the user's, so the user message usually never reached `/sync`.
  - The same bug was in gizzi's `agent-compat` sync route.
  - Both now publish the message whose id the event carries.
- In a bot chat, the header box names the bot and its pinned model, matching the footer.

## Verification
- Reproduced live against the installed Desktop gateway: Desktop-style turns 6, 7 and 8 showed only their replies, and a raw `/sync` tap showed no `role:user` event, although the rows were stored.
- `cargo test -p allternit-api --lib agent_session`: 25/25, including the new `event_message_is_the_one_the_event_names`.
- gizzi-code: `bun run typecheck` clean; `script/ci-smoke-test.sh` 145 files green, 0 failures (new `agent-compat-event-message` and `WelcomeBox` row tests).
- Live check of the new header: shows `Bot: live-check` and `Model: claude-cli/claude-opus-5`.

## Not done
- **Live re-check of the API fix, plus the rest of the checklist** (both sides typing, a forced handoff, the HUD Thread view, permission prompts answered on each side). These need a Desktop build from main that includes #961. The build was **not started**: the disk had 12 GB free (99% full), under the workspace's 50 GB gate. Eoj has to decide on cleanup first.
- The `live-check` test bot (local `~/.gizzi/bots/live-check`, plus platform agent `gizzi-bot-cf1e3ca5-…`) is kept until that live check is done.
