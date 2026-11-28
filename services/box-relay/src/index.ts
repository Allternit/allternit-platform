// Box relay: Wisp v1 over WebSocket for the browser computer (step 71,
// CONTRACT-colibri-unkernel.md section 9). NOT DEPLOYED: routing, DNS and the
// first deploy wait for Eoj's go-ahead, and RELAY_ENABLED defaults to off.
//
// Front Worker: switch, path, upgrade and Clerk check. Then the socket goes to
// a Durable Object keyed by the Clerk user id, which holds the user's caps.

import { connect } from "cloudflare:sockets";
import { DEFAULT_CLERK_JWKS_URL, parseIssuers, remoteJwks, verifyClerkToken } from "./auth";
import { makeDohResolver } from "./dns";
import { UserRelay, type ConnectFn, type LogFn, type Usage } from "./wisp";

export interface Env {
  /** Exactly "true" turns the relay on. Anything else answers 503 relay_off. */
  RELAY_ENABLED?: string;
  /** Clerk JWKS. Default https://clerk.allternit.com/.well-known/jwks.json */
  CLERK_JWKS_URL?: string;
  /** Comma-separated accepted `iss` values. Default: the two platform issuers. */
  CLERK_ISSUERS?: string;
  BOX_RELAY: DurableObjectNamespace;
}

export const WISP_PATHS = new Set(["/wisp/v1", "/wisp/v1/"]);
const USER_HEADER = "x-box-relay-user";

export interface Deps {
  verify: (token: string, env: Env) => Promise<string | null>;
}

const defaultDeps: Deps = {
  verify: (token, env) =>
    verifyClerkToken(token, {
      keys: remoteJwks(env.CLERK_JWKS_URL || DEFAULT_CLERK_JWKS_URL),
      issuers: parseIssuers(env.CLERK_ISSUERS),
    }),
};

function json(status: number, body: Record<string, string>): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", "cache-control": "no-store" },
  });
}

export async function handleRequest(request: Request, env: Env, deps: Deps = defaultDeps): Promise<Response> {
  const url = new URL(request.url);
  if (!WISP_PATHS.has(url.pathname)) return json(404, { error: "not_found" });
  if (env.RELAY_ENABLED !== "true") return json(503, { error: "relay_off" });
  if (request.headers.get("upgrade")?.toLowerCase() !== "websocket") {
    return json(426, { error: "websocket_required" });
  }

  // Checked at the upgrade only; Clerk session tokens are short-lived.
  const token = url.searchParams.get("t");
  if (!token) return json(401, { error: "unauthorized" });
  const userId = await deps.verify(token, env);
  if (!userId) return json(401, { error: "unauthorized" });

  // Forward without the query string, so the token never reaches the object.
  const headers = new Headers(request.headers);
  headers.set(USER_HEADER, userId);
  const stub = env.BOX_RELAY.get(env.BOX_RELAY.idFromName(userId));
  return stub.fetch(new Request("https://box-relay.internal/wisp/v1", { headers }));
}

export default {
  fetch(request: Request, env: Env): Promise<Response> {
    return handleRequest(request, env);
  },
} satisfies ExportedHandler<Env>;

const log: LogFn = (event) => console.log(JSON.stringify(event));

/** One per signed-in user. Holds the socket, the streams and the caps. */
export class BoxRelay {
  private readonly relay: UserRelay;

  constructor(state: DurableObjectState, _env: Env) {
    this.relay = new UserRelay({
      connect: connect as unknown as ConnectFn,
      resolve: makeDohResolver(fetch),
      store: {
        load: () => state.storage.get<Usage>("usage"),
        save: (usage) => state.storage.put("usage", usage),
      },
      log,
    });
    void state.blockConcurrencyWhile(() => this.relay.loadUsage());
  }

  async fetch(request: Request): Promise<Response> {
    if (!request.headers.get(USER_HEADER) || request.headers.get("upgrade")?.toLowerCase() !== "websocket") {
      return json(400, { error: "bad_request" });
    }
    const pair = new WebSocketPair();
    const client = pair[0];
    const server = pair[1];
    server.accept();
    const session = this.relay.open(server);
    server.addEventListener("message", (event) => session.onMessage(event.data));
    server.addEventListener("close", () => session.onSocketClosed());
    server.addEventListener("error", () => session.onSocketClosed());
    return new Response(null, { status: 101, webSocket: client });
  }
}
