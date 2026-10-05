/**
 * The slice of the Allternit Factory contract (API.md §3, v0 2026-10-04)
 * that Gizzi renders. The canonical TypeScript types live in allternit-ai
 * (`src/lib/factory/types.ts`); gizzi-code can't import across repos, so
 * these mirror the fields Gizzi reads and nothing more. Every field the
 * engine may omit is optional here so a partial engine never crashes a
 * render.
 */

export type BindingType = "hosted" | "terminal" | "vendor"

export type AgentState = "idle" | "working" | "blocked" | "needs_you" | "done" | "failed" | "offline"

export interface FactoryAgent {
  id: string
  slug?: string
  name?: string
  team?: string | null
  address?: string
  role?: string | null
  binding?: {
    type?: BindingType
    harness?: string
    vendor?: string
    mode?: "hosted" | "linked" | "mirror"
    lane?: "official" | "channel" | "ui_bridge" | "local"
    guarantee?: "exact" | "best_effort" | "read_only"
  }
  state?: AgentState
  machine?: { id: string; name: string } | null
  pane?: { id: string; attachable: boolean } | null
  currentNode?: { dagId: string; nodeId: string; title: string } | null
  proof?: { proven: number; total: number } | null
}

export interface NodeCard {
  dagId: string
  nodeId: string
  title: string
  status: "new" | "ready" | "working" | "checking" | "needs_you" | "done" | "failed" | "blocked"
  assignee?: string | null
  bindingType?: BindingType | null
  proof?: { proven: number; total: number }
  blockedBy?: string[]
  needsYou?: boolean
  depth?: number
}

export interface Board {
  campaign: { id: string; title: string; intent?: string }
  summary: {
    now: NodeCard[]
    next: NodeCard[]
    proven: { k: number; n: number }
    needsYou: NodeCard[]
  }
  waves: { depth: number; nodes: NodeCard[] }[]
}

export interface Delivery {
  id: string
  to: string
  via: string
  state: "verified" | "queued" | "best_effort" | "read_only" | "failed"
  ticket?: string | null
  threadId?: string | null
  nodeId?: string | null
  at?: string
  detail?: string | null
}
