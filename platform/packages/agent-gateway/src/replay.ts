// Recorded-session fixture format + a fake fetch that replays recorded allternit-api responses.
//
// Fixture JSON:
//   { "version": 1, "name": "...", "interactions": [
//       { "method": "POST", "path": "/threads/:id/events", "status": 200,
//         "headers": {...}?, "body": <json>, "sequence": "sticky" | "generate" } ] }
// - `path` may contain `:param` segments; queries are ignored for matching.
// - Interactions sharing method+path form a queue consumed in order; the last one sticks.
// - "generate": one interaction serves every call, with `{{n}}` (call count, from 1),
//   `{{id}}` / `{{<param>}}` (path params) and `{{body.<field>}}` (request JSON) substituted in strings.
export interface RecordedInteraction {
  method: string;
  path: string;
  status: number;
  headers?: Record<string, string>;
  body?: unknown;
  sequence?: "sticky" | "generate";
}
export interface RecordedSession { version: 1; name: string; interactions: RecordedInteraction[] }
export interface RecordedCall { method: string; path: string; url: string; body?: Record<string, unknown>; headers: Record<string, string> }

function matchPath(pattern: string, path: string): Record<string, string> | null {
  const a = pattern.split("/"), b = path.split("/");
  if (a.length !== b.length) return null;
  const params: Record<string, string> = {};
  for (let i = 0; i < a.length; i++) {
    if (a[i].startsWith(":")) params[a[i].slice(1)] = decodeURIComponent(b[i]);
    else if (a[i] !== b[i]) return null;
  }
  return params;
}

function subst(v: unknown, ctx: Record<string, unknown>): unknown {
  if (typeof v === "string") {
    return v.replace(/\{\{([\w.]+)\}\}/g, (_m, k: string) => {
      const val = k.split(".").reduce<unknown>((o, p) => (o as Record<string, unknown> | undefined)?.[p], ctx);
      return val === undefined ? "" : String(val);
    });
  }
  if (Array.isArray(v)) return v.map((x) => subst(x, ctx));
  if (v && typeof v === "object") return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, subst(x, ctx)]));
  return v;
}

export interface ReplayFetch { fetch: typeof fetch; calls: RecordedCall[]; count(method: string, pathPattern: string): number }

export function createReplayFetch(session: RecordedSession): ReplayFetch {
  const calls: RecordedCall[] = [];
  const hits = new Map<RecordedInteraction, number>();
  const queueCursor = new Map<string, number>();
  const impl = async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const url = String(input);
    const path = new URL(url, "http://replay.invalid").pathname.replace(/^\/api\/v1(?=\/)/, "");
    const method = (init?.method ?? "GET").toUpperCase();
    let body: Record<string, unknown> | undefined;
    if (typeof init?.body === "string" && init.body) { try { body = JSON.parse(init.body); } catch { /* not json */ } }
    calls.push({ method, path, url, body, headers: (init?.headers ?? {}) as Record<string, string> });

    const candidates = session.interactions.filter((i) => i.method === method && matchPath(i.path, path));
    if (!candidates.length) {
      return new Response(JSON.stringify({ error: `no recorded interaction for ${method} ${path}` }), { status: 599 });
    }
    const key = `${method} ${candidates[0].path}`;
    const idx = queueCursor.get(key) ?? 0;
    const chosen = candidates[Math.min(idx, candidates.length - 1)];
    if (chosen.sequence !== "generate") queueCursor.set(key, idx + 1);
    const n = (hits.get(chosen) ?? 0) + 1;
    hits.set(chosen, n);
    const params = matchPath(chosen.path, path) ?? {};
    const out = chosen.body === undefined ? undefined : subst(chosen.body, { n, ...params, body: body ?? {} });
    const noBody = chosen.status === 204 || out === undefined;
    return new Response(noBody ? null : JSON.stringify(out), {
      status: chosen.status,
      headers: { "content-type": "application/json", ...(chosen.headers ?? {}) },
    });
  };
  return {
    fetch: impl as typeof fetch,
    calls,
    count: (method, pattern) => calls.filter((c) => c.method === method && matchPath(pattern, c.path)).length,
  };
}

/** A fetch that fails every request in a given way (fault injection at the transport). */
export function faultFetch(kind: "vendor_down" | "network" | "rate_limited" | "auth_revoked" | "forbidden" | "drift"): typeof fetch {
  return (async () => {
    switch (kind) {
      case "network": throw new TypeError("fetch failed");
      case "vendor_down": return new Response("{}", { status: 503 });
      case "rate_limited": return new Response("{}", { status: 429, headers: { "retry-after": "7" } });
      case "auth_revoked": return new Response("{}", { status: 401 });
      case "forbidden": return new Response("{}", { status: 403 });
      case "drift": return new Response(JSON.stringify({ unexpected: "shape" }), { status: 200 });
    }
  }) as typeof fetch;
}
