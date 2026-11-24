// Public Freeweights catalog (no token). Used to badge search results.
const URL_ = "https://freeweights.allternit.com/api/models"
const TTL_MS = 5 * 60 * 1000

export interface FreeweightsEntry {
  id: string
  sha?: string
  section?: string
  license?: string
  pipeline_tag?: string
}

let cache: { at: number; entries: FreeweightsEntry[] } | undefined

/** Fetches the catalog; returns [] when unreachable so callers never fail on it. */
export async function fetchFreeweightsCatalog(): Promise<FreeweightsEntry[]> {
  if (cache && Date.now() - cache.at < TTL_MS) return cache.entries
  try {
    const resp = await fetch(URL_, { signal: AbortSignal.timeout(6000) })
    if (!resp.ok) return cache?.entries ?? []
    const data = (await resp.json()) as FreeweightsEntry[]
    const entries = Array.isArray(data) ? data.filter((e) => typeof e?.id === "string") : []
    cache = { at: Date.now(), entries }
    return entries
  } catch {
    return cache?.entries ?? []
  }
}

export async function freeweightsIds(): Promise<Set<string>> {
  return new Set((await fetchFreeweightsCatalog()).map((e) => e.id.toLowerCase()))
}
