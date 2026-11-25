import { ShareTunnelError, type GateShare, type ShareGate } from "./types.js";

/** How the caller proves itself to the runtime's `/shares` routes (allternit-api `auth_middleware`). */
export type RuntimeAuth =
  /** The signed-in user's session token, sent as `Authorization: Bearer`. */
  | { bearer: string | (() => Promise<string>) }
  /** Desktop bootstrap: `x-allternit-user-id` + `x-allternit-desktop-access-token`. */
  | { userId: string; desktopAccessToken: string | (() => Promise<string>) };

type View = {
  id: string;
  gate_url: string;
  gate_port: number;
  auth_mode: "token" | "public";
  status: "active" | "paused";
};
type Answer = { share?: View; token?: string; error?: string; message?: string; stopped?: boolean };

/**
 * The share gate over allternit-api's local routes (allternit-api `share_routes.rs`, on 127.0.0.1:18013 dev / :8013 packaged):
 * `POST /shares` `{port, subject?, auth_mode?, label?, share_link_id?}` → 201 `{share, token}`;
 * `POST /shares/:id/pause` `{paused}`, `POST /shares/:id/rotate` → `{share, token}`;
 * `POST /shares/:id/stop` or `DELETE /shares/:id` → `{stopped: true, id}` (404 `share_not_found` = already closed).
 * Errors are `{error, message}`: 403 `port_refused`, 400 `invalid_port` / `public_mcp_refused`, 409 `already_shared`, 429 `too_many_shares`.
 *
 * The per-share token is for visitors: send it as `X-Allternit-Share-Token`, never as Bearer.
 */
export function httpShareGate(options: {
  /** allternit-api on loopback, e.g. `http://127.0.0.1:8013`. */
  baseUrl: string;
  auth: RuntimeAuth;
  fetch?: typeof fetch;
}): ShareGate {
  const doFetch = options.fetch ?? fetch;
  const base = options.baseUrl.replace(/\/+$/, "");
  const resolve = async (v: string | (() => Promise<string>)) => (typeof v === "function" ? v() : v);
  const headers = async (): Promise<Record<string, string>> => {
    const a = options.auth;
    if ("bearer" in a) return { authorization: `Bearer ${await resolve(a.bearer)}` };
    return {
      "x-allternit-user-id": a.userId,
      "x-allternit-desktop-access-token": await resolve(a.desktopAccessToken),
    };
  };

  const call = async (method: string, path: string, body?: unknown, okOn404 = false): Promise<Answer> => {
    const response = await doFetch(`${base}${path}`, {
      method,
      headers: { ...(body === undefined ? {} : { "content-type": "application/json" }), ...(await headers()) },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    });
    const answer = (await response.json().catch(() => ({}))) as Answer;
    if (response.ok) return answer;
    if (okOn404 && response.status === 404) return {};
    const code = answer.error === "port_refused" ? "port_denied" : "gate_failed";
    throw new ShareTunnelError(code, answer.message ?? `Share gate answered ${response.status}`);
  };

  const toShare = (answer: Answer): GateShare => {
    if (!answer.share || !answer.token) throw new ShareTunnelError("gate_failed", "Share gate answered without a share and token.");
    const url = new URL(answer.share.gate_url);
    return {
      id: answer.share.id,
      target: `${url.hostname}:${answer.share.gate_port}`,
      token: answer.token,
      authMode: answer.share.auth_mode,
      paused: answer.share.status === "paused",
    };
  };
  const enc = encodeURIComponent;

  return {
    async open(input) {
      return toShare(
        await call("POST", "/shares", {
          port: input.port,
          subject: input.subject ?? "app",
          auth_mode: input.authMode,
          share_link_id: input.linkId,
          ...(input.label ? { label: input.label } : {}),
        }),
      );
    },
    async close(id) {
      await call("POST", `/shares/${enc(id)}/stop`, undefined, true);
    },
    async pause(id, paused = true) {
      const answer = await call("POST", `/shares/${enc(id)}/pause`, { paused });
      return answer.share?.status === "paused";
    },
    async rotate(id) {
      const answer = await call("POST", `/shares/${enc(id)}/rotate`);
      if (!answer.token) throw new ShareTunnelError("gate_failed", "Share gate rotated without a token.");
      return answer.token;
    },
  };
}
