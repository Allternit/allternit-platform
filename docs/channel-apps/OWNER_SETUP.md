# Channel apps: owner setup (Allternit's own apps)

Users can one-click connect Telegram, Discord, Slack, Teams and WhatsApp only after Allternit owns an app on each platform and the cloud API has its credentials. This is the one-time checklist. Until a channel is configured, its cloud routes return 503 `<channel>_not_configured` and the connect wizard shows **Coming soon**, so nothing is broken for users in the meantime.

Check where you are at any time:

- `GET https://api.allternit.com/api/v1/admin/channel-apps` (signed in as a user listed in `ALLTERNIT_ADMIN_USER_IDS`): per channel `configured`, `missing` env names, `webhookUrl`. Add `?check=true` to also probe each webhook URL (`webhookReachable`).
- `GET https://api.allternit.com/api/v1/channels/availability` (public): `{telegram, discord, slack, teams, whatsapp, email, phone}` booleans, what the wizards read.

## The flow, for every channel

1. Create the app in the vendor portal (steps below).
2. Store each credential in the macOS keychain: `security add-generic-password -s com.allternit.<channel>.<field> -a allternit -w` (it prompts for the value). Service names are listed per channel and by `node scripts/channel-apps/setup.mjs`.
3. `node scripts/channel-apps/setup.mjs` validates every channel against the vendor API (dry run, changes nothing). It prints the exact env names to add and writes a 0600 `channel-apps.env`. Secret values are never printed.
4. `scp channel-apps.env` to the server, merge into `/opt/allternit-cloud-api/.env`, restart cloud-api, delete the snippet.
5. `node scripts/channel-apps/setup.mjs --apply` registers webhooks / endpoints that need cloud-api to be live (Telegram webhook, Discord interactions URL).
6. Confirm with the two GET routes above.

Env names are namespaced (`ALLTERNIT_<CHANNEL>_*`). The old bare names (`APP_ID`, `APP_PASSWORD`, `TENANT_ID`, `SLACK_*`, `META_*`) still work as a fallback, but use the new ones: bare names collide in a shared `.env`.

## Telegram (about 10 minutes, no review)

- Portal: Telegram, @BotFather. `/newbot` for the manager bot, then enable Managed Bots for it in BotFather's bot settings.
- Keychain: `com.allternit.telegram.bot_token`, `com.allternit.telegram.webhook_secret` (any random 32+ char string you generate).
- Env: `ALLTERNIT_TELEGRAM_MANAGER_BOT_TOKEN`, `ALLTERNIT_TELEGRAM_MANAGER_WEBHOOK_SECRET`.
- Webhook: `https://api.allternit.com/channels/telegram-manager/<secret>`. cloud-api also sets it itself at boot; setup.mjs `--apply` does it ahead of time.
- Review wait: none. While pending, users get the paste-a-token fallback.

## Discord (about 20 minutes; verification only past 100 servers)

- Portal: https://discord.com/developers/applications, New Application.
- Bot tab: add the bot, copy the token. Gateway intents needed: Guilds, Guild Messages, Direct Messages. **Do not** enable Message Content (not requested by the code, and it needs approval past 100 servers).
- OAuth2: add redirect `https://api.allternit.com/channels/discord/oauth/callback`. Install scopes: `bot`, `applications.commands`, `identify`.
- General Information: Interactions Endpoint URL = `https://api.allternit.com/channels/discord/interactions`. Discord sends a signed PING when you save it, so cloud-api must already have the env and be running. `setup.mjs --apply` sets it after the deploy.
- Keychain fields: `app_id`, `public_key`, `bot_token`, `client_secret`.
- Env: `ALLTERNIT_DISCORD_APP_ID`, `ALLTERNIT_DISCORD_PUBLIC_KEY`, `ALLTERNIT_DISCORD_BOT_TOKEN`, `ALLTERNIT_DISCORD_CLIENT_SECRET`.
- Review wait: none under 100 servers. At 100 servers Discord requires app verification (identity check, typically days to weeks); the bot stops joining new servers at the cap until verified.
- While pending: wizard shows Coming soon until env is set; after that it works fully up to the cap.

## Slack (about 15 minutes; Marketplace review optional)

- Portal: https://api.slack.com/apps, Create New App, From a manifest, paste `docs/channel-apps/slack-app-manifest.json` (events, `/allternit` command, OAuth redirect and scopes including `files:write` (bots send files) and `files:read` (files people share are kept in their Allternit files) are in it; interactivity is off because the cloud has no block-action handler yet).
- Basic Information: copy Client ID, Client Secret, Signing Secret. Manage Distribution: activate public distribution so other workspaces can install.
- Keychain fields: `client_id`, `client_secret`, `signing_secret`; optional `bot_token` (a test install's `xoxb-` token) lets setup.mjs run `auth.test`.
- Env: `ALLTERNIT_SLACK_CLIENT_ID`, `ALLTERNIT_SLACK_CLIENT_SECRET`, `ALLTERNIT_SLACK_SIGNING_SECRET`.
- **Existing installs must re-authorize.** Slack scopes are fixed at install time: a workspace that installed before `files:write` / `files:read` was added keeps the old token, so bots can't send files and shared files can't be saved until a workspace admin re-runs the install link (Settings, Channels, Slack, Reinstall). Update the app's scopes first (re-paste the manifest under App Manifest), then ask each workspace to reinstall. Until they do, the cloud answers `slack_reauthorize_required` for incoming files and the thread shows the file name only.
- Review wait: none to install in any workspace via the OAuth link. A Slack Marketplace listing is optional and a vendor review (typically weeks); it only adds discoverability.
- While pending: users install through Allternit's own link; Slack may show the app as not Marketplace-listed.

## Microsoft Teams (about 45 minutes; publisher verification takes days)

- Portal: Azure portal, create an **Azure Bot** (or Entra app registration plus Bot Channels Registration). Choose multi-tenant (leave `ALLTERNIT_TEAMS_TENANT_ID` unset) or single-tenant (set it). Create a client secret.
- Messaging endpoint: `https://api.allternit.com/channels/teams/messages`. Enable the Microsoft Teams channel on the bot.
- Package: `scripts/channel-apps/teams-package.sh <Microsoft App ID>` produces `allternit-teams-app.zip` from `docs/channel-apps/teams/`. A tenant admin (or you, for testing) uploads it in Teams (Apps, Manage your apps, Upload) or the Teams admin center.
- Keychain fields: `app_id`, `app_password`, optional `tenant_id`.
- Env: `ALLTERNIT_TEAMS_APP_ID`, `ALLTERNIT_TEAMS_APP_PASSWORD`, optional `ALLTERNIT_TEAMS_TENANT_ID`.
- Review wait: Microsoft publisher verification (Partner Center / MPN, typically a few business days) removes the "unverified publisher" consent warning for other tenants. Teams Store (AppSource) validation is optional and typically weeks.
- While pending: customers' admins can sideload the package; consent screens show the app as unverified.

## WhatsApp via Meta (the slow one: weeks)

- Portal: https://developers.facebook.com, create a Business-type app, add the WhatsApp product, configure Embedded Signup and note its configuration id. A System User token (WhatsApp management permissions) goes in `system_token`.
- Keychain fields: `com.allternit.meta.app_id`, `app_secret`, `es_config_id`, optional `system_token`.
- Env: `ALLTERNIT_META_APP_ID`, `ALLTERNIT_META_APP_SECRET`, `ALLTERNIT_META_ES_CONFIG_ID`, optional `ALLTERNIT_META_SYSTEM_TOKEN`.
- Review wait: Meta business verification (typically days to a couple of weeks) and App Review for the WhatsApp permissions (typically days to weeks, with rejections and resubmits common) are required before businesses other than yours can complete Embedded Signup. Plan for 2-4 weeks.
- While pending: the app works only for people with a role on the app (add testers); everyone else sees Coming soon.

## Phone (Telnyx) and email

- Phone: `ALLTERNIT_TELNYX_API_KEY` (and `ALLTERNIT_TELNYX_PUBLIC_KEY` for webhook signatures); carrier webhook `https://api.allternit.com/api/v1/phone/webhooks/telnyx`. Number registration for US texting (10DLC / toll-free verification) has its own carrier wait, typically days to weeks.
- Email: no owner app credentials in cloud-api; always reported available.

Vendor wait times above are typical figures, not guarantees; they are controlled by the vendors.
