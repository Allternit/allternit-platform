# abacusai-bot-web

Abacus.AI bot through the web app (`apps.abacus.ai/bot/`), the same surface the
Abacus.AI web client uses. Route: browser sign-in.

- Sign in: open `https://apps.abacus.ai/bot/` in Allternit's login browser with
  your own Abacus.AI account. Allternit never reads cookies from your normal browser.
- Chat: `chat.create` / `chat.continue`, driven by the shared web-chat base.
- Status: v1-unverified. Selectors and the login probe were written without a
  logged-in session. Probe the live page, then fix `selectors/v1.yaml`.
- Not used: the Abacus.AI API key (`/v1/abacusaibot_channels`). That API only
  carries the owner's WhatsApp inbox; it cannot list bots or send to a chosen bot.
- Host RPC (`/api/botHost/<id>/rpc`) needs a connect token the platform mints from
  its own secret. A third-party client cannot connect to it directly.
