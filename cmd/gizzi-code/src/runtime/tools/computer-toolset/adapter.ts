/**
 * Computer toolset: which adapter a model gets, and the tool schema/description
 * each adapter sends. One contract (contract.gen.ts), one executor
 * (allternit-api POST /api/v1/computers/:id/toolset), many adapters:
 *
 * - claude-native: Anthropic models get the native `computer_toolset_20260801`
 *   / `browser_toolset_20260801` entries. The AI SDK (@ai-sdk/anthropic 2.0.x)
 *   has no toolset type, so gizzi registers the function tool below and the
 *   provider fetch hook (anthropic-native.ts) swaps it for the native entry on
 *   the wire and maps the toolset tool_use blocks back. If the API rejects the
 *   native entry for a model, the same request is resent with the function tool
 *   (the claude-function fallback).
 * - openai / gemini / json-function: JSON function tools generated from the
 *   contract. @ai-sdk/openai 2.0.89 and @ai-sdk/google 2.0.54 cannot parse
 *   OpenAI `computer_call` items or Gemini `computer_use` function calls into
 *   the loop's tool calls, so their native computer tools are not used yet.
 * - grid: GUI-trained open models (UI-TARS, Qwen-VL) answer in a 0..1000 grid;
 *   the executor maps it (coordinate_space "normalized_1000"). Scaling never
 *   happens here.
 */
import z from "zod/v4"
import { CONTRACTS, type ToolsetName, type ToolsetMemberSpec } from "./contract.gen"

export type AdapterKind = "claude-native" | "openai-function" | "gemini-function" | "json-function"

export interface AdapterChoice {
  kind: AdapterKind
  /** 0..1000 normalized coordinates instead of model-frame pixels. */
  grid: boolean
}

export interface ModelRef {
  providerID: string
  modelID: string
  npm?: string
}

/** Marker the Anthropic wire hook uses to recognise our function tools. */
export const TOOL_MARKER: Record<ToolsetName, string> = {
  computer: "[allternit.computer.v1]",
  browser: "[allternit.browser.v1]",
}

const GRID_MODEL = /ui-?tars|qwen[\d.]*-?vl|qwen\d*\.?\d*-vl|glm-4\.?\dv|cogagent|showui|os-atlas/i

export function chooseAdapter(model?: ModelRef): AdapterChoice {
  const forcedGrid = process.env.GIZZI_COMPUTER_TOOLSET_GRID
  const id = `${model?.providerID ?? ""}/${model?.modelID ?? ""}`
  const grid = forcedGrid === "1" || forcedGrid === "true" ? true : forcedGrid === "0" ? false : GRID_MODEL.test(id)
  const npm = model?.npm ?? ""
  if (npm === "@ai-sdk/anthropic" || /(^|\/)(anthropic|claude)/i.test(id) || /claude/i.test(model?.modelID ?? "")) {
    return { kind: "claude-native", grid: false }
  }
  if (npm === "@ai-sdk/openai" || /^openai\//i.test(id)) return { kind: "openai-function", grid }
  if (npm === "@ai-sdk/google" || /gemini/i.test(id)) return { kind: "gemini-function", grid }
  return { kind: "json-function", grid }
}

/** Zod for the JSON-schema subset the contract uses. */
export function toZod(schema: Record<string, any>): z.ZodType {
  if (Array.isArray(schema.anyOf)) {
    const options = schema.anyOf.map((s: Record<string, any>) => toZod(s))
    return options.length === 1 ? options[0] : z.union(options as [z.ZodType, z.ZodType, ...z.ZodType[]])
  }
  if (schema.const !== undefined) return z.literal(schema.const)
  if (Array.isArray(schema.enum)) return z.enum(schema.enum as [string, ...string[]])
  let out: z.ZodType
  switch (schema.type) {
    case "string":
      out = z.string()
      break
    case "number":
      out = z.number()
      break
    case "integer":
      out = z.number().int()
      break
    case "boolean":
      out = z.boolean()
      break
    case "array": {
      let arr = z.array(toZod(schema.items ?? {}))
      if (typeof schema.minItems === "number") arr = arr.min(schema.minItems)
      if (typeof schema.maxItems === "number") arr = arr.max(schema.maxItems)
      out = arr
      break
    }
    case "object": {
      const required = new Set<string>(schema.required ?? [])
      const shape: Record<string, z.ZodType> = {}
      for (const [k, v] of Object.entries<Record<string, any>>(schema.properties ?? {})) {
        shape[k] = required.has(k) ? toZod(v) : toZod(v).optional()
      }
      out = z.object(shape)
      break
    }
    default:
      out = z.any()
  }
  return schema.description ? out.describe(schema.description) : out
}

/**
 * The function-tool parameters for one toolset: `action` (the member) plus
 * every member's fields, all optional (the executor validates each call
 * against its member schema). Field names and types are the contract's, so a
 * call maps 1:1 onto `{ member: action, input: rest }`.
 */
export function toolParameters(toolset: ToolsetName, members: readonly ToolsetMemberSpec[]) {
  const fields: Record<string, z.ZodType> = {}
  for (const m of members) {
    const props = (m.input_schema as any).properties ?? {}
    for (const [k, v] of Object.entries<Record<string, any>>(props)) {
      if (!fields[k]) fields[k] = toZod(v).optional()
    }
  }
  const names = members.map((m) => m.name) as [string, ...string[]]
  return z.object({
    action: z.enum(names).describe(`Which ${toolset} action to run.`),
    ...fields,
  })
}

export function enabledMembers(toolset: ToolsetName, enabled?: Set<string>): ToolsetMemberSpec[] {
  return CONTRACTS[toolset].members.filter((m) => (enabled ? enabled.has(m.name) : m.default_enabled))
}

export function toolDescription(toolset: ToolsetName, choice: AdapterChoice, members: readonly ToolsetMemberSpec[]): string {
  const frame = choice.grid
    ? "Coordinates are on a 0-1000 grid over the whole screen on both axes (0,0 top-left, 1000,1000 bottom-right)."
    : "Coordinates are pixels in the frame of the most recent screenshot (origin top-left). Take a screenshot first."
  const lines = members.map((m) => `- ${m.name}: ${m.description}`)
  const what =
    toolset === "computer"
      ? "Use a computer's screen, mouse and keyboard."
      : "Use a web browser session: navigate, look at the page, click, type and read text."
  return [
    `${TOOL_MARKER[toolset]} ${what} Set \`action\` to one of the actions below and pass that action's fields.`,
    frame,
    "Calls in one turn run in order; after a failure the rest are not executed.",
    "Actions that need a person's approval pause until they answer.",
    "",
    ...lines,
  ].join("\n")
}
