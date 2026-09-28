# @allternit/computer-embed

Embeddable, read-only live-computer widget for Allternit Cloud Computer
(spec `rq-20260909-004`, Phase 5 — distribution surface). A small
framework-free RFB-over-WebSocket viewer plus a `<allternit-computer>` web
component. No React, no noVNC dependency, no build step required to serve it.

Viewers are **read-only by construction**: the client has no code path for
keyboard, mouse, or clipboard input, and embed tokens are forced read-only on
the server as well. Interactive control of a paired machine lives in Fabric
Session Desktop (`https://fabrictransport.allternit.com`), not in this embed.

## Quick start

Serve this directory (`embed.html` + `dist/`) from any static host, then:

```html
<script type="module" src="https://your-static-host/embed/dist/embed-page.js"></script>
<allternit-computer host="https://api.example.com" computer="cmp_…"
                    token="…" hosts="api.example.com"
                    style="height:360px"></allternit-computer>
```

Or iframe the standalone page (same query params):

```html
<iframe src="https://your-static-host/embed/embed.html?host=https://api.example.com&computer=cmp_…&token=…"
        style="width:640px;height:360px;border:0" loading="lazy"></iframe>
```

Element attributes / query params:

| Name | Meaning |
|---|---|
| `src` | Full `ws(s)://…/ws/computers/:id/vnc` URL. Overrides `host`+`computer`. |
| `host` | API origin, e.g. `https://api.example.com`. `https` → `wss://`. |
| `computer` | Computer id. |
| `token` | Embed token (see below). Sent only as the `?token=` query param. |
| `hosts` | Space-separated hostname allowlist for the connection target. |
| `label` | Status-bar text; defaults to `view-only`. |

If `hosts` is set, a target whose hostname is not listed is refused. If it is
omitted, any host is allowed — the same default the API server uses for its
viewer page CSP (`ALLTERNIT_EMBED_FRAME_ANCESTORS`, default `*`). Set both.

## Minting an embed token

Tokens are minted **server-side, by the computer's owner, with their normal
API credentials** — never in browser code, never hardcoded into a page that
ships to users. One token authorizes both the viewer page and the VNC stream;
it is bound to a single computer and expires in 15 minutes.

```bash
curl -X POST "https://api.example.com/api/v1/computers/cmp_…/embed-token" \
  -H "Authorization: Bearer <owner session token>"
```

Response:

```json
{
  "token": "eyJ…",
  "expires_in": 900,
  "vnc_ws_url": "/ws/computers/cmp_…/vnc?token=eyJ…",
  "viewer_url": "/embed/computers/cmp_…?token=eyJ…"
}
```

Token rules:

- **Short-lived by design** (`expires_in: 900`). Mint per page render / per
  viewer session; do not cache or reuse across users.
- **Read-only scoped.** The API signs embed tokens `read_only: true` and the
  WebSocket proxy forces read-only for every purpose-`embed` token
  (`vnc_readonly` filter), so even a leaked or tampered token cannot inject
  input.
- **It travels in URLs**, so it ends up in access logs. The 15-minute TTL is
  the mitigation — keep it.
- A stopped or deleted computer rejects the stream; re-mint after restart.

## How it works

- `src/rfb-client.ts` — minimal RFB 3.x client over binary WebSocket. Version
  exchange, None-auth (the proxy injects the guest VNC password and presents
  security type `[1]` to viewers), SecurityResult, ClientInit, ServerInit.
  Decodes **raw** and **copyrect** encodings plus the desktop-size
  pseudo-encoding (server-initiated resize). Hextile/tight/zrle are not
  implemented — the client only advertises what it decodes.
- `src/allternit-computer.ts` — the web component: shadow-DOM canvas, status
  bar, `hosts` allowlist enforcement.
- `src/embed-page.ts` + `embed.html` — the iframe-able page; config comes from
  the query string.
- Dark theme matches `#0b0b0c`.

The API server also ships its own noVNC-based viewer page at
`/embed/computers/:id?token=…` (vendored assets, frozen server code). That
page is view-only for embed tokens; opened with an approval-gated
`ws-token {"purpose":"vnc","read_only":false}` instead, it is interactive
(keyboard/mouse) unless someone else holds the control lease. This
package is the standalone, dependency-light variant for embedding from static
hosts; both speak the same wire protocol.

## Development

```bash
npm install        # devDeps only: typescript, vitest, ws
npm run typecheck  # tsc --noEmit
npm test           # vitest: unit + mock-RFB integration tests
npm run build      # regenerate dist/ (committed; embed.html depends on it)
```

Tests run headless in Node: a mock RFB 3.8 server (`test/mock-rfb-server.ts`)
implements the exact None-auth wire behavior of the API proxy and serves a
known pixel pattern; the integration test asserts the handshake, raw and
copyrect rendering, incremental updates, fragmented delivery, and that the
client never sends input message types (4/5/6).

## Limitations

- Raw + copyrect only. Busy desktops on slow links will use more bandwidth
  than a hextile/tight viewer.
- View-only: no keyboard, mouse, or clipboard, by design.
- No audio, no multiple monitors (first framebuffer only).
