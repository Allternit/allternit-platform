/**
 * Bot names for the pods pane. `GET /api/v1/floor/state` returns bot ids only;
 * Desktop and web get names from the agents list (`GET /api/v1/agents`, shown
 * as `botProfile.displayName ?? name`, see allternit-ai getBotDisplayName).
 * This does the same lookup once per call. Unknown ids fall back to the id,
 * and a failed lookup never breaks the pane.
 */
import { platformRequest } from "@/runtime/bots/platform-api"

type AgentRow = { id?: unknown; name?: unknown; botProfile?: { displayName?: unknown } | null }

const text = (v: unknown): string | undefined => (typeof v === "string" && v.trim() ? v.trim() : undefined)

export async function botNames(
  ids: string[],
  fetchAgents: () => Promise<{ agents?: AgentRow[] }> = () =>
    platformRequest<{ agents?: AgentRow[] }>("GET", "/api/v1/agents?is_bot=true", undefined, { timeoutMs: 10_000 }),
): Promise<Map<string, string>> {
  const names = new Map<string, string>()
  let rows: AgentRow[] = []
  try {
    rows = (await fetchAgents()).agents ?? []
  } catch {
    // Offline or signed out: show ids.
  }
  const byId = new Map<string, string>()
  for (const r of rows) {
    const id = text(r.id)
    const name = text(r.botProfile?.displayName) ?? text(r.name)
    if (id && name) byId.set(id, name)
  }
  for (const id of ids) names.set(id, byId.get(id) ?? id)
  return names
}
