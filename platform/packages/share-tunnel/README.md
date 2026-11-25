# @allternit/share-tunnel

The one live-link client (decision D4) for Desktop share-manager, agent-daemon and `gizzi share`. It serves
a local port through the allternit-api share gate and the self-hosted OpenTunnel relay
(`infra/opentunnel/`, v0.6.2 + admission patch), and heartbeats cloud-api so pause, revoke and kill stop it.

```ts
import { createShareTunnel, httpShareGate } from "@allternit/share-tunnel";

const tunnel = createShareTunnel({
  relayApi: "https://api.live.allternitusercontent.com",
  cloudApi: "https://api.allternit.com",
  getAuthToken: () => session.token(),
  gate: httpShareGate({ baseUrl: "http://127.0.0.1:8013", auth: { bearer: () => session.token() } }),
  onEvent: (event) => log(event),
});
const link = await cloudApi.createShareLink({ kind: "live", subject: "port", target_port: 3000, runtime_device_id });
const share = await tunnel.start({ link, port: 3000, admission: link.relay_credential });
share.url;            // https://<subdomain>.live.allternitusercontent.com
await share.stop();   // or it stops itself: heartbeat 410 → "killed"/"revoked", paused → "paused"
```

- Gate contract (allternit-api `share_routes.rs`): `POST /shares {port, subject, auth_mode: "token"|"public", share_link_id}`
  → `{share, token}`; `pause`, `rotate`, `stop` (`DELETE` also). `share.token` (`ast_...`) is what visitors send, as
  `X-Allternit-Share-Token` (or `?allternit_share_token=` once), never Bearer. `share.pause()` / `share.rotate()` call the gate.
  Runtime auth: `{ bearer }` (session) or `{ userId, desktopAccessToken }` (Desktop bootstrap). `signed_in`/`org` links use `token`.
- Ports 8013, 18013 (allternit-api) and 4096 (gizzi) are refused here and again in the gate.
- One OpenTunnel profile per link (profile = subdomain) under `dataDir` (default `~/.allternit/share-tunnel`).
  The first start on a device needs the relay credential; later starts reuse the saved identity.
- Heartbeat: `POST {cloudApi}/api/v1/share-links/:id/heartbeat` every 60 s. 410 stops and deletes the local
  identity; a 200 with `status: "paused"` stops with reason `paused` and deletes it too (cloud-api drops the
  relay tunnel on pause, so resume starts again with a fresh credential from
  `POST /api/v1/share-links/:id/relay-credential`); network errors keep serving.
- Bytes are not reported by the client: the OpenTunnel SDK exposes none, and D8 meters only relay-side
  bytes (`connection.closed` analytics → cloud-api `share_link_usage`). `status().connections` is live.
- Needs Node 22+ (or Bun 1.4+), the SDK's floor.

Tests: `pnpm --filter @allternit/share-tunnel test`. The unit tests use fakes. The relay test runs only
with the env from `infra/opentunnel/scripts/local-relay.sh` (MySQL in docker, a cargo build): train only.
