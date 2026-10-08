// Allternit computer / browser toolset contract, typed. Everything here is
// generated from contracts/computer-toolset/*.json except the helpers below.
export * from "./generated.js"
import { CONTRACTS, type ToolsetMemberSpec, type ToolsetName } from "./generated.js"

/** The contract row for one member, or undefined when it does not exist. */
export function memberSpec(toolset: ToolsetName, member: string): ToolsetMemberSpec | undefined {
  return CONTRACTS[toolset].members.find((m) => m.name === member)
}

/** Members a toolset offers by default (before per-target executor limits). */
export function defaultEnabledMembers(toolset: ToolsetName): string[] {
  return CONTRACTS[toolset].members.filter((m) => m.default_enabled).map((m) => m.name)
}

/** The exact text a skipped call returns after an earlier call in the same turn failed. */
export function batchHaltText(toolset: ToolsetName): string {
  return CONTRACTS[toolset].batch_halt_text
}
