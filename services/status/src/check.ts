// Cloudflare-vantage probes: status code + body marker + latency, all in parallel.
import type { ProbeResult, Target } from "../contract/types";

export const DEFAULT_TIMEOUT_MS = 8000;
const UA = "allternit-status/1 (+https://status.allternit.com)";

export type Fetcher = (input: string, init?: RequestInit) => Promise<Response>;

export async function probe(t: Target, fetcher: Fetcher = fetch, clock: () => number = Date.now): Promise<ProbeResult> {
  const ctrl = new AbortController();
  const timeout = Math.min(t.timeout_ms ?? DEFAULT_TIMEOUT_MS, 20_000);
  const timer = setTimeout(() => ctrl.abort(), timeout);
  const start = clock();
  try {
    const res = await fetcher(t.url, {
      method: t.method,
      redirect: t.follow_redirects ? "follow" : "manual",
      headers: { "user-agent": UA, "cache-control": "no-cache" },
      signal: ctrl.signal,
    });
    let markerOk: boolean | null = null;
    if (t.body_marker !== null && t.method !== "HEAD") {
      const body = await res.text();
      markerOk = body.includes(t.body_marker);
    } else {
      // Release the connection without reading it.
      await res.body?.cancel().catch(() => undefined);
    }
    const latency = Math.max(0, Math.round(clock() - start));
    const codeOk = res.status === t.expect_status;
    const ok = codeOk && markerOk !== false;
    return {
      target_id: t.id,
      ok,
      status_code: res.status,
      latency_ms: latency,
      marker_ok: markerOk,
      error: ok ? null : !codeOk ? `status_${res.status}` : "marker_missing",
    };
  } catch (e) {
    const aborted = ctrl.signal.aborted || (e instanceof Error && e.name === "AbortError");
    return {
      target_id: t.id,
      ok: false,
      status_code: null,
      latency_ms: null,
      marker_ok: null,
      error: aborted ? "timeout" : `fetch_${e instanceof Error ? e.message.slice(0, 80) : "error"}`,
    };
  } finally {
    clearTimeout(timer);
  }
}

export function probeAll(targets: Target[], fetcher: Fetcher = fetch): Promise<ProbeResult[]> {
  return Promise.all(targets.map((t) => probe(t, fetcher)));
}
