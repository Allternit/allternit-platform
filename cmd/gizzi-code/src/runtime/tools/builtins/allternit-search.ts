/**
 * Web search through Allternit's own route (cloud-api POST /api/v1/search/web),
 * for any model. Used first by the engine's `websearch` tool and the TUI's
 * WebSearch fallback; signed out, or when the route can't answer, callers fall
 * back to the public Exa MCP endpoint as before.
 *
 * The route is a metered search on Allternit's account, signed in with the
 * same credentials as artifacts/sessions: ALLTERNIT_API_KEY / ALLTERNIT_API_TOKEN
 * or the paired device token.
 */
import { CLOUD_URLS } from "@/shared/constants/cloudUrls"
import { Pairing } from "@/runtime/services/pairing/pairing"

export type AllternitSearchResult = { title: string; url: string; published?: string | null; text?: string }

export namespace AllternitSearch {
  export const PATH = "/api/v1/search/web"

  export function base(): string {
    const explicit =
      process.env.ALLTERNIT_CLOUD_API_URL?.trim() || process.env.GIZZI_PLATFORM_API_URL?.trim() || CLOUD_URLS.api
    return explicit.replace(/\/+$/, "")
  }

  async function tokens(): Promise<string[]> {
    if (["1", "true"].includes((process.env.GIZZI_DISABLE_ALLTERNIT_SEARCH ?? "").toLowerCase())) return []
    const env = [process.env.ALLTERNIT_API_KEY?.trim(), process.env.ALLTERNIT_API_TOKEN?.trim()]
    const stored = await Pairing.load().catch(() => undefined)
    const device = Pairing.tokenUsable(stored) ? stored.deviceToken : undefined
    return [...new Set([...env, device].filter((t): t is string => !!t))]
  }

  /**
   * Results from Allternit's route, or undefined when there is no credential
   * or the route can't answer (rejected, not deployed, not configured, network).
   */
  export async function search(
    query: string,
    opts: { numResults?: number; signal?: AbortSignal; fetch?: typeof fetch; tokens?: string[] } = {},
  ): Promise<AllternitSearchResult[] | undefined> {
    const list = opts.tokens ?? (await tokens())
    const doFetch = opts.fetch ?? fetch
    for (const token of list) {
      try {
        const response = await doFetch(`${base()}${PATH}`, {
          method: "POST",
          headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json", Accept: "application/json" },
          body: JSON.stringify({ query, num_results: opts.numResults ?? 8 }),
          signal: opts.signal ?? AbortSignal.timeout(25_000),
        })
        if (response.status === 401 || response.status === 403) continue
        if (!response.ok) return undefined
        const json = (await response.json()) as { results?: AllternitSearchResult[] }
        return Array.isArray(json.results) ? json.results.filter(r => r && typeof r.url === "string") : undefined
      } catch (error) {
        if (opts.signal?.aborted) throw error
        return undefined
      }
    }
    return undefined
  }

  /** The text shape the Exa MCP returns, so source pills and parsers keep working. */
  export function toText(results: AllternitSearchResult[]): string {
    return results
      .map(r =>
        [`Title: ${r.title || r.url}`, `URL: ${r.url}`, r.published ? `Published Date: ${r.published}` : "", r.text ? `Text: ${r.text}` : ""]
          .filter(Boolean)
          .join("\n"),
      )
      .join("\n\n")
  }
}
