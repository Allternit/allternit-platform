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
import { PermissionNext } from "@/runtime/tools/guard/permission/next"

const log = Log.create({ service: "computer-toolset" })

/**
 * Permission class of an access ask (request_access, or an access_required
 * step). Always asks, in every mode: an app grant is the person's own act.
 */
export const COMPUTER_ACCESS_PERMISSION = "computer.access"
PermissionNext.ALWAYS_ASK.add(COMPUTER_ACCESS_PERMISSION)

const TIER_RANK: Record<string, number> = { read: 0, click: 1, full: 2 }

/** One app in an access ask (resolve_access entry plus the tier asked for). */
export interface AccessEntry {
  name: string
  bundle_id: string
  category?: string
  max_tier?: string
  tier: string
}

/** The `access` block the executor puts on access_required / request_access approval_required replies. */
interface AccessBlock {
  entries?: Array<Partial<AccessEntry>>
  reason?: string
  needed_tier?: string
  granted_tier?: string
  expires_in_secs?: number
}

function accessOf(body: unknown): AccessBlock | undefined {
  const access = (body as { access?: unknown } | undefined)?.access
  return access && typeof access === "object" ? (access as AccessBlock) : undefined
}

function textOf(body: ToolsetResult & { message?: string }): string {
  return (body.content ?? [])
    .filter((c) => c.type === "text")
    .map((c) => (c as { text: string }).text)
    .join("\n")
}

function accessSummary(entries: AccessEntry[]): string {
  return `Let the agent use ${entries.map((e) => `${e.name || e.bundle_id} (${e.tier})`).join(", ")}`
}

/** Model frame per session + toolset: the size of the last screenshot the model saw. */
const frames = new Map<string, { width: number; height: number }>()
/** Next call index per turn (assistant message). */
const callIndexes = new Map<string, number>()

export function targetComputer(): string {
  return process.env.GIZZI_COMPUTER_ID?.trim() || "this-device"
}

/**
 * The bot or platform agent this runtime runs as (the env the bot runner
 * sets), for the agent cursor badge. Plain gizzi sessions send neither and
 * the executor falls back to the computer's own bot.
 */
export function runningAgent(env: Record<string, string | undefined> = process.env): { agent_id?: string; agent_name?: string } {
  const agent_id = (env.ALLTERNIT_AGENT_ID || env.ALLTERNIT_BOT_ID)?.trim() || undefined
  const agent_name = (env.ALLTERNIT_AGENT_NAME || env.ALLTERNIT_BOT_NAME)?.trim().slice(0, 60) || undefined
  return { agent_id, agent_name }
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
 * One toolset call through the executor, including the approval round trips:
 * - access_required (the app in front holds a lower tier): one computer.access
 *   ask, request_access with that answer's grant, then the step once more;
 * - 409 approval_required asks the person (ctx.ask; request_access asks
 *   computer.access with its apps, tiers and reason), approves the grant and
 *   retries the same call once with it (`approval_grant`);
 * - tier_exceeded comes back as a clear error, with nothing asked.
 */
export async function runToolsetCall(
  toolset: ToolsetName,
  args: Record<string, unknown>,
  ctx: Pick<Tool.Context, "sessionID" | "messageID" | "abort" | "ask">,
  deps: RunDeps,
): Promise<ToolsetOutput> {
  const { action, step, ...input } = args
  const member = String(action)
  for (const [k, v] of Object.entries(input)) if (v === undefined) delete input[k]
  const frameKey = `${ctx.sessionID}:${toolset}`
  const actor = runningAgent()
  const stepText = typeof step === "string" ? step.trim().slice(0, 120) : ""
  const req: ToolsetRequest = {
    toolset,
    member,
    input,
    run_id: ctx.sessionID,
    turn_id: ctx.messageID,
    call_index: nextCallIndex(`${ctx.messageID}:${toolset}`),
    model_frame: frames.get(frameKey),
    coordinate_space: deps.grid ? "normalized_1000" : "pixels",
    ...(actor.agent_id ? { agent_id: actor.agent_id } : {}),
    ...(actor.agent_name ? { agent_name: actor.agent_name } : {}),
    ...(stepText ? { step: stepText } : {}),
  }
  let reply = await deps.executor.run(deps.computerId, req, ctx.abort)
  // The app in front holds a lower tier than this step needs: ask the person
  // for that tier, record it with request_access, then run the step once more.
  if (reply.body.error === "access_required" && accessOf(reply.body)?.entries?.length) {
    await grantAccessFor(member, req.call_index, reply.body, stepText, ctx, deps)
    reply = await deps.executor.run(deps.computerId, req, ctx.abort)
  }
  if (reply.status === 409 && reply.body.error === "approval_required" && reply.body.approval_id) {
    const access = member === "request_access" ? accessOf(reply.body) : undefined
    if (access) {
      // request_access: the card shows the apps, tiers and reason.
      const entries = accessEntries(access)
      await ctx.ask({
        permission: COMPUTER_ACCESS_PERMISSION,
        patterns: entries.map((e) => `${e.bundle_id}:${e.tier}`),
        always: [],
        metadata: {
          summary: accessSummary(entries),
          toolset,
          member,
          computer: deps.computerId,
          access: { entries, reason: access.reason ?? "", expires_in_secs: access.expires_in_secs },
        },
      })
    } else {
      await ctx.ask({
        permission: `computer_toolset.${toolset}`,
        patterns: [`${toolset}.${member}`],
        always: [],
        metadata: { toolset, member, input, risk: reply.body.risk, computer: deps.computerId },
      })
    }
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
    const message = textOf(body)
    log.info("toolset call failed", { toolset, member, status: reply.status, error: body.error })
    if (body.error === "tier_exceeded") throw new Error(tierExceeded(message))
    throw new Error(message || body.message || `${member} failed (HTTP ${reply.status})`)
  }
  return renderResult(toolset, member, body)
}

function tierExceeded(message: string): string {
  return [
    `tier_exceeded: ${message || "An app was asked for a tier above its ceiling."}`,
    "Nothing was granted and the person was not asked. Call resolve_access to see each app's max_tier and ask for that tier or less; for a step that needs more, ask the person to do it.",
  ].join(" ")
}

function accessEntries(access: AccessBlock, tier?: string): AccessEntry[] {
  return (access.entries ?? [])
    .filter((e) => typeof e.bundle_id === "string" && e.bundle_id)
    .map((e) => ({
      name: e.name || e.bundle_id!,
      bundle_id: e.bundle_id!,
      ...(e.category ? { category: e.category } : {}),
      ...(e.max_tier ? { max_tier: e.max_tier } : {}),
      tier: tier ?? e.tier ?? "read",
    }))
}

/**
 * The access round trip for a step refused with access_required: one ask
 * (computer.access, the app and the tier the step needs), then request_access
 * on the person's behalf (its own approval is recorded from that same answer)
 * so the grant exists in the executor's session. A tier above the app's
 * ceiling never asks: the step's own message says why it can't run.
 */
async function grantAccessFor(
  member: string,
  callIndex: number | undefined,
  body: ToolsetResult & { message?: string },
  stepText: string,
  ctx: Pick<Tool.Context, "sessionID" | "messageID" | "abort" | "ask">,
  deps: RunDeps,
): Promise<void> {
  const access = accessOf(body)!
  const needed = access.needed_tier ?? "click"
  const entries = accessEntries(access, needed)
  const message = textOf(body)
  const over = entries.find((e) => e.max_tier !== undefined && (TIER_RANK[needed] ?? 2) > (TIER_RANK[e.max_tier] ?? 0))
  if (!entries.length || over) throw new Error(message || `${member} needs ${needed} access, above what this app can be granted. It did not run.`)
  const reason = (stepText || `${member} needs ${needed} access`).slice(0, 200)
  await ctx.ask({
    permission: COMPUTER_ACCESS_PERMISSION,
    patterns: entries.map((e) => `${e.bundle_id}:${e.tier}`),
    always: [],
    metadata: {
      summary: accessSummary(entries),
      toolset: "computer",
      member,
      computer: deps.computerId,
      access: { entries, reason, needed_tier: needed, granted_tier: access.granted_tier, expires_in_secs: access.expires_in_secs },
    },
  })
  const request: ToolsetRequest = {
    toolset: "computer",
    member: "request_access",
    input: { entries: entries.map((e) => ({ bundle_id: e.bundle_id, tier: e.tier })), reason },
    run_id: ctx.sessionID,
    turn_id: ctx.messageID,
    // The step's own index: the executor halts a turn's later calls after a
    // failed one, and this request stands in for the refused step.
    call_index: callIndex,
    coordinate_space: "pixels",
  }
  let reply = await deps.executor.run(deps.computerId, request, ctx.abort)
  if (reply.status === 409 && reply.body.error === "approval_required" && reply.body.approval_id) {
    if (!(await deps.executor.approve(reply.body.approval_id, ctx.abort))) {
      throw new Error(`Access was approved here but the approval couldn't be recorded; ${member} did not run.`)
    }
    reply = await deps.executor.run(deps.computerId, { ...request, approval_grant: reply.body.approval_id }, ctx.abort)
  }
  if (reply.body.is_error || reply.status >= 400) {
    const why = textOf(reply.body)
    if (reply.body.error === "tier_exceeded") throw new Error(tierExceeded(why))
    throw new Error(`Access for ${member} couldn't be granted: ${why || reply.body.message || `HTTP ${reply.status}`}. It did not run.`)
  }
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
