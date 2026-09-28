/**
 * The worker fetch serves the in-process server's URL (http://gizzi.internal)
 * over RPC, and returns each response only once its whole body has arrived.
 * Everything else (model providers, MCP, telemetry) keeps the native fetch:
 * routed through the worker, a provider's SSE stream reached the TUI in one
 * piece at the end, so replies, reasoning and the spinner never streamed.
 */
export function routeInternalFetch(workerFetch: typeof fetch, internalUrl: string | undefined, networkFetch: typeof fetch): typeof fetch {
  if (!internalUrl) return networkFetch
  const origin = new URL(internalUrl).origin
  const fn = (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url
    let target: string
    try {
      target = new URL(url).origin
    } catch {
      return networkFetch(input, init)
    }
    return target === origin ? workerFetch(input, init) : networkFetch(input, init)
  }
  return Object.assign(fn, networkFetch) as typeof fetch
}
