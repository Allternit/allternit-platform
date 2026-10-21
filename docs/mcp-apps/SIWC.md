# Sign in with ChatGPT (SIWC)

Lets a user of Allternit Desktop use their ChatGPT plan for ChatGPT-model requests instead of an API key. **The flag is off by
default, it is Desktop-only, and OpenAI approval is pending.** Do not enable it for anyone other than the developer until OpenAI approves.

## Why approval matters

OpenAI's SIWC docs cover open-source, locally run apps. Allternit Desktop is a paid product with accounts, which OpenAI directs to its
interest form. The repo's licence files disagree about Desktop (`surfaces/allternit-desktop/package.json` says `UNLICENSED`, the installer
text reads MIT-style, the root `package.json` says MIT), so do not assume Desktop counts as open source.

## How it works

- Flag `feature.siwc` (`surfaces/allternit-desktop/src/main/feature-flags.ts`), default `false`. Enable with `ALLTERNIT_FLAG_FEATURE_SIWC=1` or
  `"feature.siwc": true` in `~/.allternit/flags.json`, then restart Desktop. With the flag off, no network call, secret read or browser launch happens.
- Flow (`src/main/siwc.ts`): dynamic client registration (`dynamic_agent_client`, no secret), authorization code + PKCE through the system
  browser with a `127.0.0.1` loopback callback, ID-token validation (JWKS, issuer, audience, expiry, nonce). Issuer `https://auth.openai.com`.
- Token custody: Desktop's main process only. Refresh happens 60 s before expiry, serialized. Sign out revokes.
- `gizzi-code` never holds the refresh token. Desktop runs a loopback broker (`siwc-broker.ts`) and passes `ALLTERNIT_SIWC_BROKER_URL` and
  `ALLTERNIT_SIWC_BROKER_TOKEN` to the gizzi child it spawns; gizzi asks it for an access token per request and calls
  `POST https://api.openai.com/v1/responses`.
- When signed in, the provider takes the existing `subs-chatgpt` lane. It falls back to the web-chat adapter only before any output and on
  not-signed-in, network failure, 401 or 5xx. Usage-limit and ineligible-user errors go to the user.

## Not done

- No live sign-in against auth.openai.com; tested against a fake OpenAI only.
- Text and image input only: no tools, no reasoning-summary stream. Context and output limits are constants.
- One active account in the UI; no account picker.
- Host id is a `urn:uuid:`, not the recommended JWK-thumbprint form.
- The always-on launchd gizzi daemon does not get the broker env, so SIWC is unavailable when Desktop attaches to it.
- Button branding and OpenAI's UI/UX guidelines have not been reviewed. The Settings card is in the private `allternit-ai` repo.
