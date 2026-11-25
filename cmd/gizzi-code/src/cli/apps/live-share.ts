/**
 * `gizzi share <port>`: a live link from this computer (Apps + MCP 2026, lane 43b).
 * Creates the cloud-api live link, then serves it with @allternit/share-tunnel through the local
 * allternit-api share gate. Runs in the foreground; Ctrl-C stops it. `gizzi share list|stop|pause`
 * act on the same link in cloud-api, and the running process stops on its next heartbeat.
 */
import { randomBytes } from "node:crypto"
import { createShareTunnel, httpShareGate, ShareTunnelError } from "@allternit/share-tunnel"
import { cloudInferenceToken, cloudOrigin } from "@/runtime/providers/discovery/allternit-cloud"
import { cloud, fail, AppsError, type ShareLink } from "@/cli/apps/client"

const RELAY_API = (process.env.ALLTERNIT_SHARE_RELAY_API || "https://api.live.allternitusercontent.com").trim()
const LOCAL_API = (process.env.ALLTERNIT_LOCAL_API_URL || "http://127.0.0.1:8013").trim()

export async function shareLive(port: number): Promise<void> {
  try {
    const token = async () => {
      const t = await cloudInferenceToken().catch(() => undefined)
      if (!t) throw new AppsError("unauthorized", 401, "Not signed in. Run `gizzi login`, or set ALLTERNIT_API_TOKEN.")
      return t
    }
    await token()
    const made = await cloud<{ link: ShareLink; relay_credential?: string }>("POST", "/api/v1/share-links", {
      kind: "live",
      subject: "app",
      target_port: port,
      auth_mode: "token",
      subdomain: `l${randomBytes(5).toString("hex")}`,
      label: `localhost:${port}`,
    })
    const tunnel = createShareTunnel({
      relayApi: RELAY_API,
      cloudApi: cloudOrigin(),
      getAuthToken: token,
      gate: httpShareGate({ baseUrl: LOCAL_API, auth: { bearer: token } }),
    })
    let live
    try {
      live = await tunnel.start({
        link: { id: made.link.id, subdomain: made.link.subdomain },
        port,
        authMode: "token",
        admission: made.relay_credential,
      })
    } catch (e) {
      // Nothing is serving this link: revoke it rather than leave a dead one counted against the cap.
      await cloud("DELETE", `/api/v1/share-links/${encodeURIComponent(made.link.id)}`).catch(() => undefined)
      if (e instanceof ShareTunnelError && e.code === "gate_failed")
        throw new Error(`${e.message} Is allternit-api running at ${LOCAL_API}?`)
      throw e
    }
    process.stdout.write(`${live.url}\nToken: ${live.token}\n(visitors send it as X-Allternit-Share-Token)\nSharing localhost:${port}. Ctrl-C to stop.\n`)
    const stop = () => void live.stop("stopped")
    process.once("SIGINT", stop)
    process.once("SIGTERM", stop)
    const reason = await live.stopped
    process.stdout.write(`Stopped (${reason}).\n`)
    // Ctrl-C ends the link; a pause or an admin kill from cloud-api keeps its own state.
    if (reason === "stopped") await cloud("DELETE", `/api/v1/share-links/${encodeURIComponent(made.link.id)}`).catch(() => undefined)
    process.exit(0)
  } catch (e) {
    fail(e)
  }
}
