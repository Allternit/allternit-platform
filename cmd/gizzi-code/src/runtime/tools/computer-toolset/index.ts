/**
 * The `computer`, `computer_v2` and `browser` tools: the Allternit computer
 * toolset (allternit.computer.v2's 17 pixel members + its 7 structured
 * members, allternit.browser.v1) for every model family.
 *
 * Each call goes to the one executor, allternit-api
 * `POST /api/v1/computers/:id/toolset`, which validates it against the
 * contract, checks the control lease and approvals, writes the audit row,
 * scales coordinates and dispatches. This module never scales coordinates.
 *
 * Target computer: GIZZI_COMPUTER_ID (default `this-device`).
 * See adapter.ts for which adapter each model gets.
 */
import { Tool } from "@/runtime/tools/builtins/tool"
import { Log } from "@/shared/util/log"
import { CONTRACTS, type ToolsetName, type ToolsetRequest, type ToolsetResult } from "./contract.gen"
import { chooseAdapter, enabledMembers, enabledV2Members, toolDescription, toolDescriptionV2, toolParameters, V2_STRUCTURED_MEMBERS, type ModelRef } from "./adapter"
import { httpExecutor, type ExecutorClient } from "./executor-client"
import { BROWSER_STATE_CLOSE, BROWSER_STATE_OPEN } from "./anthropic-native"

const log = Log.create({ service: "computer-toolset" })

/** Model frame per session + toolset: the size of the last screenshot the model saw. */
const frames = new Map<string, { width: number; height: number }>()
/** Next call index per turn (assistant message). */
const callIndexes = new Map<string, number>()

export function targetComputer(): string {
  return process.env.GIZZI_COMPUTER_ID?.trim() || "this-device"
}

function nextCallIndex(turn: string): number {
  const i = callIndexes.get(turn) ?? 0
  callIndexes.set(turn, i + 1)
  if (callIndexes.size > 500) callIndexes.delete(callIndexes.keys().next().value!)
  return i
}

export interface ToolsetOutput {
  title: string
  output: string
  metadata: Record<string, unknown>
  attachments?: Array<{ type: "file"; mime: string; url: string; filename?: string }>
}

/** Turn an executor result into the tool output gizzi returns to the model. */
export function renderResult(toolset: ToolsetName, member: string, result: ToolsetResult): ToolsetOutput {
  const texts = result.content.filter((c) => c.type === "text").map((c) => (c as { text: string }).text)
  const images = result.content.filter((c) => c.type === "image") as Array<{ media_type: string; data: string }>
  if (result.browser_state) texts.push(`${BROWSER_STATE_OPEN}${JSON.stringify(result.browser_state)}${BROWSER_STATE_CLOSE}`)
  return {
    title: `${toolset} ${member}`,
    output: texts.join("\n") || (images.length ? "Screenshot taken." : "OK"),
    metadata: { toolset, member, screen: result.screen, truncated: false },
    attachments: images.map((img, i) => ({
      type: "file" as const,
      mime: img.media_type,
      url: `data:${img.media_type};base64,${img.data}`,
      filename: `${member}-${i + 1}.png`,
    })),
  }
}

export interface RunDeps {
  executor: ExecutorClient
  computerId: string
  grid: boolean
}

/**
 * One toolset call through the executor, including the approval round trip:
 * a 409 approval_required asks the person (ctx.ask), approves the grant and
 * retries the same call once with it.
 */
export async function runToolsetCall(
  toolset: ToolsetName,
  args: Record<string, unknown>,
  ctx: Pick<Tool.Context, "sessionID" | "messageID" | "abort" | "ask">,
  deps: RunDeps,
): Promise<ToolsetOutput> {
  const { action, ...input } = args
  const member = String(action)
  for (const [k, v] of Object.entries(input)) if (v === undefined) delete input[k]
  const frameKey = `${ctx.sessionID}:${toolset}`
  const req: ToolsetRequest = {
    toolset,
    member,
    input,
    run_id: ctx.sessionID,
    turn_id: ctx.messageID,
    call_index: nextCallIndex(`${ctx.messageID}:${toolset}`),
    model_frame: frames.get(frameKey),
    coordinate_space: deps.grid ? "normalized_1000" : "pixels",
  }
  let reply = await deps.executor.run(deps.computerId, req, ctx.abort)
  if (reply.status === 409 && reply.body.error === "approval_required" && reply.body.approval_id) {
    await ctx.ask({
      permission: `computer_toolset.${toolset}`,
      patterns: [`${toolset}.${member}`],
      always: [],
      metadata: { toolset, member, input, risk: reply.body.risk, computer: deps.computerId },
    })
    if (!(await deps.executor.approve(reply.body.approval_id, ctx.abort))) {
      throw new Error(`${member} was approved here but the approval couldn't be recorded; it did not run.`)
    }
    reply = await deps.executor.run(deps.computerId, { ...req, approval_grant: reply.body.approval_id }, ctx.abort)
  }
  const body = reply.body
  if (body.screen?.frame_width && body.screen?.frame_height) {
    frames.set(frameKey, { width: body.screen.frame_width, height: body.screen.frame_height })
  }
  if (body.is_error || reply.status >= 400) {
    const message = (body.content ?? [])
      .filter((c) => c.type === "text")
      .map((c) => (c as { text: string }).text)
      .join("\n")
    log.info("toolset call failed", { toolset, member, status: reply.status, error: body.error })
    throw new Error(message || body.message || `${member} failed (HTTP ${reply.status})`)
  }
  return renderResult(toolset, member, body)
}

/** Members the target actually runs (executor schema), cached for 5 minutes. */
const schemaCache = new Map<string, { at: number; enabled?: Set<string> }>()
async function targetMembers(toolset: ToolsetName, computerId: string): Promise<Set<string> | undefined> {
  const key = `${computerId}:${toolset}`
  const hit = schemaCache.get(key)
  if (hit && Date.now() - hit.at < 5 * 60_000) return hit.enabled
  const schema = await httpExecutor.schema(computerId, toolset, AbortSignal.timeout(3_000)).catch(() => undefined)
  const enabled = schema?.members?.length ? new Set<string>(schema.members.filter((m) => m.enabled).map((m) => m.name as string)) : undefined
  // An empty set (target offline) falls back to the contract defaults so the
  // model still gets a clear per-call error instead of a missing tool.
  const usable = enabled && enabled.size > 0 ? enabled : undefined
  schemaCache.set(key, { at: Date.now(), enabled: usable })
  return usable
}

function defineToolset(toolset: ToolsetName) {
  return Tool.define(toolset, async (init?: Tool.InitContext) => {
    const choice = chooseAdapter(init?.model as ModelRef | undefined)
    const members = enabledMembers(toolset, await targetMembers(toolset, targetComputer()))
    const parameters = toolParameters(toolset, members)
    return {
      description: toolDescription(toolset, choice, members),
      parameters,
      async execute(args: Record<string, unknown>, ctx: Tool.Context) {
        return runToolsetCall(toolset, args, ctx, { executor: httpExecutor, computerId: targetComputer(), grid: choice.grid })
      },
    }
  })
}

/**
 * The v2 structured members as their own function tool, for every model
 * family: Claude gets this next to its native toolset (the wire hook only
 * rewrites the `computer` tool's v1 marker, so this stays a plain function
 * tool); OpenAI, Gemini and everyone else get it next to `computer`.
 */
function defineV2Toolset() {
  return Tool.define("computer_v2", async () => {
    const members = enabledV2Members(await targetMembers("computer", targetComputer()))
    return {
      description: toolDescriptionV2(members),
      parameters: toolParameters("computer", members),
      async execute(args: Record<string, unknown>, ctx: Tool.Context) {
        // The executor's contract is "computer"; v2 members are members of it.
        return runToolsetCall("computer", args, ctx, { executor: httpExecutor, computerId: targetComputer(), grid: false })
      },
    }
  })
}

/** `computer`: a computer's screen, mouse and keyboard (17 pixel members). */
export const ComputerToolsetTool = defineToolset("computer")
/** `computer_v2`: the structured driver-backed members (read_ui, act, run_batch, verify, request_human, use_credential, run_subtask). */
export const ComputerV2ToolsetTool = defineV2Toolset()
/** `browser`: a browser session (31 members; executor reports which run). */
export const BrowserToolsetTool = defineToolset("browser")

export const TOOLSET_MEMBER_COUNTS = {
  computer: CONTRACTS.computer.members.length,
  computer_v2: V2_STRUCTURED_MEMBERS.length,
  browser: CONTRACTS.browser.members.length,
}
