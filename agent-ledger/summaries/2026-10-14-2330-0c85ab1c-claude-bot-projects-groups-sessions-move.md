# Bot-project connectors, vendor group chat, Sessions computer move — 2026-10-14

Session 0c85ab1c (Claude), finishing Codex's "bot project coverage" run at Eoj's request.

## What was done
- **Connectors for the 33 pictured bot projects** (platform #1518, #1528; ai #538, #545):
  - **Native API adapters:** OpenMausBot, Codync, Hivekeep, Invisible Dots, CopilotKit OpenDots / OpenMuse / OpenBot, DS Bot, Nightly OpenBot, Rakazo, FrockBot, Sidemates, nanoMuse, Vellum Assistant, Open Dots (Anil), Gawkbot, Munder Difflin, Happy.
  - **Browser sign-in:** Gemini Spark, Slashy, AbacusAI.
  - **macOS Messages lane:** Iris, Poke, Instinct, Flip, Cue, Pally.
  - **Existing vendors:** Grok Bot, OpenAI Dots, Muse, Cadu→Hermes.
  - **On hold (Eoj):** Noodle (QUIC Hub link needs a Swift bridge; WIP parked in Allternit/Research/bot-project-coverage/parked), Omnibots (closed, no interface).
- **Vendor bots in group rooms** (ai #545): vendor members answer inline through their connector with the room history, on one standing thread per room (`vendor-group-turn.ts`). This fixed GroupChatView running vendor bots on a native model. Each connector also got its own look pack (brand accent + logo; Messages face for text assistants).
- **Routing fix** (#1528): `adapter_for_auth` routes bot-project accounts by vendor id (`DIRECT_ADAPTER_VENDORS`). Before, account discovery for them failed `UNSUPPORTED`.
- **Merge fix:** removed the duplicate `IdentityStepProps` left by the #538 merge.
- **Sessions gateway under systemd** (#1530): it now survives a reboot or move.
- **Infra (Eoj-approved):**
  - Sessions container: raised 2→4 CPU, 4→8 GiB (host load 87), then moved mail VPS → allternit-standby (export/import, same name). Desktop `incus-host.env` now points at `allternit-standby` over Tailscale.
  - Hermes runs as a system service.
  - Deleted 3 empty AllternitOS VMs, `allternit-build`, the stopped free-project container, and cartlidge.joseph's empty standby computer (its provisioned_instances row was retired with the same steps as `ProvisioningService::delete`).
  - `host_standby` totals were reduced to reserve the Sessions share.
- **Security:** the mail VPS published a Sessions VNC display with no password on 0.0.0.0. It was closed at once. On standby, ports 30000–30199 are mesh-only (`allt-sessions-fw.service`).

## Verification
- **Tests:** adapter type checks; ai typecheck plus 208 files / 1716 tests (bots, gateway, bot-phone, agent-view); `cargo test -p allternit-api` gateway_connect + agent_gateway_routes; docs `check_links` 0 problems.
- **Deploys:** ai.allternit.com and m.allternit.com on ba0fd5c. Desktop runtime r20261014.2002-b59ad3f-323 (published by another job from the same commits). Sessions gateway redeployed; health 200 through Desktop.
- **Reboot test:** two container reboots; the gateway and Hermes return on their own.

## Honest deferrals
- No connector has been exercised against a live vendor account.
- Browser-lane selectors are v1 guesses.
- Messages lane is macOS-only; the Sessions gateway is Linux, so those connectors are inert until a Mac gateway exists. Eoj: not needed now.
- ChatGPT and Claude on Sessions show `challenge_presented` after the IP change. Eoj clicks through.
- `host_mail` stays the capped Platform API hosted-driver pool. It was briefly disabled by mistake and restored within about 10 minutes.
