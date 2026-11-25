import { ShareTunnelError, type ShareGate } from "./types.js";

/**
 * The share gate over allternit-api's local routes (plan v2 step 16):
 * `POST {baseUrl}/shares` `{ link_id, subdomain, port, auth_mode }` → 201 `{ id, target: "127.0.0.1:<gate port>" }`
 * (403 `{ error: "port_denied" }` for internal ports, 401 without the token);
 * `DELETE {baseUrl}/shares/:id` → 204 (404 = already closed).
 */
export function httpShareGate(options: {
  /** allternit-api on loopback, e.g. `http://127.0.0.1:8013`. */
  baseUrl: string;
  token: string | (() => Promise<string>);
  fetch?: typeof fetch;
}): ShareGate {
  const doFetch = options.fetch ?? fetch;
  const base = options.baseUrl.replace(/\/+$/, "");
  const auth = async () =>
    `Bearer ${typeof options.token === "function" ? await options.token() : options.token}`;
  return {
    async open(input) {
      const response = await doFetch(`${base}/shares`, {
        method: "POST",
        headers: { "content-type": "application/json", authorization: await auth() },
        body: JSON.stringify({
          link_id: input.linkId,
          subdomain: input.subdomain,
          port: input.port,
          auth_mode: input.authMode,
        }),
      });
      const body = (await response.json().catch(() => ({}))) as {
        id?: string;
        target?: string;
        error?: string;
        message?: string;
      };
      if (!response.ok || !body.id || !body.target) {
        const code = body.error === "port_denied" ? "port_denied" : "gate_failed";
        throw new ShareTunnelError(code, body.message ?? `Share gate answered ${response.status}`);
      }
      return { id: body.id, target: body.target };
    },
    async close(id) {
      const response = await doFetch(`${base}/shares/${encodeURIComponent(id)}`, {
        method: "DELETE",
        headers: { authorization: await auth() },
      });
      if (!response.ok && response.status !== 404) {
        throw new ShareTunnelError("gate_failed", `Share gate answered ${response.status}`);
      }
    },
  };
}
