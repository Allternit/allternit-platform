/**
 * Data for the factory floor (`/factory`, `f`). Every row comes from the
 * Allternit Factory engine: `agents ps --json`, `workspace board --json`,
 * `workspace approve <node>`. Nothing is cached, merged or invented here —
 * a section the engine can't answer carries the engine's own message, and
 * the screen shows "—" with it.
 *
 * Kept free of ink imports so tests drive it against a fake engine script.
 */
import { FactoryEngineError, runEngine } from '@/cli/factory/engine'
import type { Board, FactoryAgent, NodeCard } from '@/cli/factory/types'

export type Section<T> =
  | { ok: true; data: T }
  | {
      ok: false
      /** The engine reported the verb as specified but not built yet. */
      notBuilt: boolean
      /** The engine's fact, unchanged. */
      message: string
      action: string | null
      exitCode: number
    }

export interface FactoryFloorData {
  agents: Section<FactoryAgent[]>
  board: Section<Board>
}

export type EngineRunner = typeof runEngine

/** Per-call budget: a hung engine must not freeze the TUI. */
export const FLOOR_TIMEOUT_MS = 8000

function failed<T>(err: unknown): Section<T> {
  const e =
    err instanceof FactoryEngineError
      ? err
      : new FactoryEngineError({ code: "transport", fact: (err as Error)?.message || String(err) })
  return { ok: false, notBuilt: e.notBuiltYet, message: e.fact, action: e.action, exitCode: e.exitCode }
}

export async function loadAgents(run: EngineRunner = runEngine): Promise<Section<FactoryAgent[]>> {
  try {
    const { data } = await run<{ agents?: FactoryAgent[] }>(["agents", "ps"], { timeoutMs: FLOOR_TIMEOUT_MS })
    if (!data || !Array.isArray(data.agents)) {
      return failed(
        new FactoryEngineError({ code: "transport", fact: "The engine's `agents ps` reply has no agents list" }),
      )
    }
    return { ok: true, data: data.agents }
  } catch (err) {
    return failed(err)
  }
}

export async function loadBoard(campaign?: string, run: EngineRunner = runEngine): Promise<Section<Board>> {
  try {
    const args = ["workspace", "board", ...(campaign ? [campaign] : [])]
    const { data } = await run<Board>(args, { timeoutMs: FLOOR_TIMEOUT_MS })
    if (!data || !data.summary || !data.campaign) {
      return failed(new FactoryEngineError({ code: "transport", fact: "The engine's `workspace board` reply isn't a board" }))
    }
    return { ok: true, data }
  } catch (err) {
    return failed(err)
  }
}

export async function loadFactoryFloor(campaign?: string, run: EngineRunner = runEngine): Promise<FactoryFloorData> {
  const [agents, board] = await Promise.all([loadAgents(run), loadBoard(campaign, run)])
  return { agents, board }
}

/** Approve a node waiting on a person. Resolves to a one-line outcome. */
export async function approveNode(
  node: Pick<NodeCard, "nodeId" | "title">,
  run: EngineRunner = runEngine,
): Promise<{ ok: boolean; message: string }> {
  try {
    await run(["workspace", "approve", node.nodeId], { timeoutMs: FLOOR_TIMEOUT_MS })
    return { ok: true, message: `Approved ${node.nodeId} · ${node.title}` }
  } catch (err) {
    const s = failed(err) as Extract<Section<unknown>, { ok: false }>
    return { ok: false, message: s.action ? `${s.message} — ${s.action}` : s.message }
  }
}

/**
 * A Vendor bot has no terminal: its work arrives as tickets on node
 * deliveries. Show what the engine has for its thread.
 */
export async function loadVendorThread(address: string, run: EngineRunner = runEngine): Promise<Section<unknown>> {
  try {
    const { data } = await run(["orchestration", "transcript", address], { timeoutMs: FLOOR_TIMEOUT_MS })
    return { ok: true, data }
  } catch (err) {
    return failed(err)
  }
}

/** What Enter does for a bot, by binding (SPEC §10 "Gizzi (terminal)"). */
export type OpenAction =
  | { kind: "attach"; address: string }
  | { kind: "session"; name: string }
  | { kind: "vendor"; address: string }
  | { kind: "none"; reason: string }

export function openActionFor(agent: FactoryAgent, address: string): OpenAction {
  const type = agent.binding?.type
  if (type === "terminal") {
    if (agent.pane && agent.pane.attachable === false) return { kind: "none", reason: `${address} has no attachable pane right now` }
    return { kind: "attach", address }
  }
  if (type === "hosted") return { kind: "session", name: agent.slug ?? agent.name ?? agent.id }
  if (type === "vendor") return { kind: "vendor", address }
  return { kind: "none", reason: `The engine didn't say how ${address} runs` }
}
