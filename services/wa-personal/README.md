# wa-personal — WhatsApp personal number (unofficial)

Runtime-local Node sidecar that links a personal WhatsApp number as a **linked device (QR)** using
[Baileys](https://github.com/WhiskeySockets/Baileys) (MIT, pinned `6.7.24`). It runs on the user's own
runtime (Desktop or cloud computer), never in the Allternit cloud. Off unless `ALLTERNIT_WA_PERSONAL=1`.

> **Unofficial. WhatsApp can ban numbers that automate messaging.** Use a dedicated number you can
> afford to lose. The API returns this warning on `/session/start`, `/session/qr`, `/status`, and every
> disabled-state 503, so the UI can show it in red.

## Env

| var | |
|---|---|
| `ALLTERNIT_WA_PERSONAL=1` | required; otherwise the process exits 0 and any handler answers 503 `wa_personal_disabled` |
| `ALLTERNIT_WA_PERSONAL_TOKEN` | required; bearer for the local API and the `x-allternit-sidecar-token` sent to allternit-api |
| `ALLTERNIT_WA_PERSONAL_KEY` | optional 32-byte key (64 hex / base64) sealing session files; else a random key file `<data>/wa-personal.key` (0600) |
| `ALLTERNIT_WA_PERSONAL_OWNERS` | optional comma list of extra pre-approved numbers (digits) |
| `ALLTERNIT_WA_PERSONAL_PORT` | default `8791`, bound to `127.0.0.1` only |
| `ALLTERNIT_API_URL` | default `http://127.0.0.1:8013` |
| `ALLTERNIT_WA_PERSONAL_DIR` | default `$ALLTERNIT_DATA_DIR/wa-personal` (`~/.allternit/wa-personal`) |

## Run / test

```
npm install && ALLTERNIT_WA_PERSONAL=1 ALLTERNIT_WA_PERSONAL_TOKEN=… npm start
npm test        # node:test, Baileys mocked, no install needed
```

## HTTP API (all need `Authorization: Bearer $ALLTERNIT_WA_PERSONAL_TOKEN`)

See `docs/C_WA_PERSONAL_NOTES.md` for exact JSON.
