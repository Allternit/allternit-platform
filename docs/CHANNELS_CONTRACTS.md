# Channels swarm: shared rules and contracts (read this before your task file)

The orchestrator is joe-07 (Claude). Plan: `/Users/joe/Desktop/Allternit/Allternit Brain/Research/specs/channel-packs.md`, sections v3, v3.1 and v3.2. It was approved by Eoj on 2026-10-02. About nine Kimi executors run in parallel, each in its own worktree. **Stay inside your file ownership** so the merges stay clean.

## Production model (applies to everyone)

- **Inbound.** Platforms post to the cloud: `api.allternit.com/channels/in/<key>` (`cmd/allternit-cloud-api/src/routes/channel_inbound.rs`). The cloud queues the request and relays it to the user's runtime, which is either a cloud computer (woken if it sleeps) or a Desktop. `target_path(provider)` maps a provider to its runtime path.
- **Instant acknowledgements.** Some platforms need a signed answer within 3 seconds: Discord PING, Slack url_verification, Meta's verify handshake. Others need their token checked at the edge: Teams' Bot Framework JWT. These are answered or checked **in the cloud** before anything is queued.
- **Secrets.** A **shared Allternit app's** secrets (Discord bot token, Slack signing secret, Teams app password, Telnyx key, Meta system token, the Telegram manager token) live **only in cloud-api env vars**. They never reach a runtime, a log or Postgres in plain text. Per-user tokens are sealed with `token_crypto`, as today.
- **Outbound for shared apps.** The runtime asks the cloud to send. Each provider adds its own cloud route `POST /api/v1/channels/<provider>/send`, authenticated as the user the same way `channel-inbound-routes` is. The runtime's transport for that provider calls it.
- **Threads.**
  - Each external conversation is one bot thread, through `thread_routes::channel_thread` / `channel_thread_under`.
  - Routing reuses `route_inbound` member-bot logic in `channel_transports.rs`: default bot, "@name" sub-threads, and the switched-off-bot notice.
  - Each provider adds its own mention forms (for example "@Allternit name", `/name`, or a reply to a bot's message) in **its own module**, through a small hook. Don't rewrite `route_inbound`.
- **Not configured.** If a provider's env is unset, its routes return 503 `{error:"<provider>_not_configured"}`, so the UI can fall back. Never crash at boot.
- **Events.** Write `channel.*` events as today. Calls use `call.*`, a contract that is frozen in `HANDOFF-realtime-voice-2026-10-02.md` §4/§4.1.

## Migration numbers (assigned, don't take another's)

| Executor | allternit-api (sqlite) | cloud-api (`migrations_pg`) |
|---|---|---|
| ao-mail-reply | V212 | — |
| ao-tg-managed | V213 | 022 |
| ao-phone-sms | V214 | 024 |
| ao-voice-cloud | — | 023 |
| ao-discord | V215 | 025 |
| ao-slack | V216 | 026 |
| ao-teams | V217 | 027 |
| ao-whatsapp | V218 | 028 |

## Files everyone may touch (add only, minimal lines; conflicts are resolved at merge)

- Router registration lines in `cmd/allternit-api/src/main.rs` / `lib.rs` and the cloud-api `lib.rs`.
- `target_path` / `channel_headers` in `channel_inbound.rs`: one match arm or header for your provider.
- `PROVIDERS` and `build_transport` in `channel_transports.rs`: your arm only.
- allternit-ai `CHANNEL_FIELDS` / `CONNECTABLE`: your provider only.

Everything else lives in **new files you own**, named after your provider. Never edit another executor's module.

## Hands off

- `voice_calls.rs`, `services/voice/**`, LiveKit deploys: these belong to the voice session joe-9e.
- `agent_email_routes.rs` / `services/mailflare`: these belong to ao-mail-reply.

## Every executor

- **Budget.** At most **300k tokens and 180 tool calls**. If you reach either, stop and write your notes file. You may use your own subagents for parallel subtasks inside your scope.
- **No real credentials.** Don't call real third-party APIs with real credentials. Use a fake HTTP layer in tests, and copy how the existing channel tests fake `HttpSend`.
- **Verify the docs.** Check the vendor's current official docs for every field and endpoint name, and cite the URL in your notes. If the docs contradict your task, stop and note it. Don't invent names.
- **Rust tests:** `CARGO_TARGET_DIR=/Users/joe/Desktop/allternit-workspace/.gw-target cargo test -p <crate> --lib <your module>`. The build cache is shared, so expect to wait for its lock sometimes.
- **allternit-ai checks:** `npx vitest run <your files>` and `npx tsc --noEmit -p .`. Report only errors in your files.
- **Smoke-boot.** If you add routes or migrations, boot the binary once on a fresh data dir or DB. Also grep the routers for path overlaps; route overlaps have crash-looped prod before.
- **Commits.** Small commits that say why, on your `ao/<slug>` branch. End each message with `Co-Authored-By: Kimi <noreply@moonshot.ai>`. No push, no merge, no deploy.
- **UI.**
  - Allternit tokens and the existing gateway components: white surfaces with `--neutral-fill`, never tan, no vendor chrome.
  - Phone and SMS layouts may follow universal phone conventions.
  - Respect reduced motion and keyboard access.
  - Read `/Users/joe/.agents/skills/libraries-dev/SKILL.md` before adding loading or voice effects.
- **Deliverable.** `docs/<TOPIC>_NOTES.md` covering:
  - the files changed;
  - the verified API names, with URLs;
  - the exact JSON of every new endpoint;
  - test commands and their results;
  - what's left;
  - a last line of exactly `status: done` or `status: blocked: <reason>`.

  Then `touch docs/<TOPIC>_NOTES.sentinel`.

## MERGE PROTOCOL: end your session with the merge (Eoj, 2026-10-02 21:30)

When your feature is done, merge it yourself. Follow these steps exactly:

1. Commit everything. Then `git fetch origin main && git rebase origin/main` and fix any conflicts. For conflicts in the shared add-only spots (router lines, `target_path`, `PROVIDERS`, `CHANNEL_FIELDS`), keep both sides.

2. **Take the merge lock** so merges happen one at a time:
   `exec 9>/Users/joe/Desktop/allternit-workspace/scratch/channels-swarm/merge.lock && flock 9`.
   If `flock` is missing, use `lockf` or a mkdir lock at `.../merge.lock.d`, with a retry loop every 30s. Hold the lock until step 6.

3. With the lock held, fetch and rebase again. Then run the gates. **All of them must pass:**
   - **Rust:**
     - your module tests;
     - `cargo test -p allternit-api --lib channel_` (plus `agent_email` if you touched email);
     - the cloud-api crate tests for your module;
     - `cargo build --release -p allternit-api` and the cloud-api binary.
   - **Smoke-boot:** if you added routes or migrations, start allternit-api on a fresh temp `ALLTERNIT_DATA_DIR` for 15 s. It must not panic, have route overlaps, or hit migration errors. Do the same for cloud-api if it can boot without Postgres; otherwise run its migration SQL against a throwaway database, or document why you couldn't.
   - **Migrations:** your migration number must still be the one assigned to you and must not exist on main.
   - **allternit-ai:** `npx vitest run` for your files and the gateway folders, plus `npx tsc --noEmit -p .` with no new errors in your files.

4. Push the branch: `git push -u origin HEAD`. Open the PR with `gh pr create`. The title is the feature; the body covers what, why, tests, the env vars needed to turn it on, and "behind unset env = 503". End it with:
   `🤖 Generated with [Claude Code](https://claude.com/claude-code)` (for Claude sessions) or `Co-Authored-By: Kimi` (for Kimi).

5. Merge: `gh pr merge <n> --merge`. Wait for GitHub to say it merged. Merges to `cmd/allternit-api` auto-deploy to prod. Your feature must be inert while its env vars are unset.

6. Release the lock. Write your NOTES file (including the PR number and merge commit), with the last line `status: done`. Touch the sentinel. Then **end the session**.

**If any gate fails and you can't fix it within your budget,** don't merge. Write `status: blocked: <gate + error>` and stop. The orchestrator, joe-07, fixes forward.

**Never:**
- force-push to main;
- merge someone else's PR;
- edit another executor's files to make your gates pass;
- run prod database migrations. Prod cloud migrations are applied by hand later, so list them in your notes.
