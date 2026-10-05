/**
 * Pure row formatting for the factory floor. Badges and proof text reuse
 * the CLI renderer so `gizzi agents ps` and the TUI say the same thing.
 */
import { agentAddress, bindingBadge, proofText } from "@/cli/factory/render"
import type { FactoryAgent, NodeCard } from "@/cli/factory/types"

export const NOT_BUILT_MARK = "—"

export interface AgentRowSegments {
  address: string
  badge: string
  state: string
  /** Theme color name for the state. */
  stateColor: "success" | "warning" | "error" | "suggestion" | "subtle"
  node: string
  proof: string
}

export function stateThemeColor(state: string | undefined): AgentRowSegments["stateColor"] {
  switch (state) {
    case "working":
    case "checking":
      return "suggestion"
    case "done":
      return "success"
    case "needs_you":
    case "blocked":
      return "warning"
    case "failed":
      return "error"
    default:
      return "subtle"
  }
}

export function buildAgentRow(agent: FactoryAgent): AgentRowSegments {
  return {
    address: agentAddress(agent),
    badge: bindingBadge(agent),
    state: (agent.state ?? NOT_BUILT_MARK).replace("_", " "),
    stateColor: stateThemeColor(agent.state),
    node: agent.currentNode?.title ?? NOT_BUILT_MARK,
    proof: proofText(agent.proof),
  }
}

export function nodeRowText(n: NodeCard): string {
  const who = n.assignee ? ` · ${n.assignee}` : ""
  return `${n.nodeId}  ${n.title}  ${proofText(n.proof)}${who}`
}

export function truncate(text: string, max: number): string {
  if (max <= 0) return ""
  if (text.length <= max) return text
  return `${text.slice(0, Math.max(0, max - 1))}…`
}
