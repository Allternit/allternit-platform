import { gateway } from "@/cli/work/api"
import { botNames } from "@/cli/work/bot-names"
import type { FloorState } from "@/cli/work/pods-model"

/**
 * Pods = the Office floor's grouping (allternit-api `/api/v1/floor/state`, floor v2).
 * The floor returns bot ids only, so names come from the agents list, the same
 * source Desktop uses; an id with no name stays an id.
 */
export async function getFloorState(): Promise<FloorState> {
  const state = await gateway<FloorState>("/api/v1/floor/state")
  const ids = new Set<string>()
  for (const b of state.bots ?? []) ids.add(b.id)
  for (const p of state.pods ?? []) for (const id of p.botIds ?? []) ids.add(id)
  if (!ids.size) return state
  const names = await botNames([...ids])
  const known = new Map((state.bots ?? []).map((b) => [b.id, b]))
  for (const id of ids) if (!known.has(id)) known.set(id, { id })
  return { ...state, bots: [...known.values()].map((b) => ({ ...b, name: b.name ?? names.get(b.id) })) }
}
