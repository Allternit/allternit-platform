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
  gate: httpShareGate({ baseUrl: "http://127.0.0.1:8013", token: localApiToken }),
  onEvent: (event) => log(event),
});
const link = await cloudApi.createShareLink({ kind: "live", subject: "port", target_port: 3000, runtime_device_id });
const share = await tunnel.start({ link, port: 3000, admission: link.relay_credential });
share.url;            // https://<subdomain>.live.allternitusercontent.com
await share.stop();   // or it stops itself: heartbeat 410 → "killed"/"revoked", paused → "paused"
```

- Ports 8013, 18013 (allternit-api) and 4096 (gizzi) are refused here and again in the gate.
- One OpenTunnel profile per link (profile = subdomain) under `dataDir` (default `~/.allternit/share-tunnel`).
  The first start on a device needs the relay credential; later starts reuse the saved identity.
- Heartbeat: `POST {cloudApi}/api/v1/share-links/:id/heartbeat` every 60 s. 410 stops and deletes the local
  identity; a link with `status: "paused"` stops but keeps it; network errors keep serving.
- Bytes are not reported by the client: the OpenTunnel SDK exposes none, and D8 meters only relay-side
  bytes (`connection.closed` analytics → cloud-api `share_link_usage`). `status().connections` is live.
- Needs Node 22+ (or Bun 1.4+), the SDK's floor.

Tests: `pnpm --filter @allternit/share-tunnel test`. The unit tests use fakes. The relay test runs only
with the env from `infra/opentunnel/scripts/local-relay.sh` (MySQL in docker, a cargo build): train only.
