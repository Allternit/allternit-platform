/**
 * second_opinion: advisory review of a plan or diff by the read-only
 * code-reviewer agent, run as a subagent (the way task.ts spawns them) on the
 * deepest thinking level the session's model supports. The answer is advice,
 * never a gate. Shapes in runtime/primitives/gap-types.ts.
 */
import { Tool } from "@/runtime/tools/builtins/tool"
import {
  SECOND_OPINION_TOOL_ID,
  SecondOpinionInput,
  type SecondOpinionMetadata,
  type SecondOpinionVerdict,
} from "@/runtime/primitives/gap-types"
import DESCRIPTION from "@/runtime/tools/builtins/second-opinion.txt"
import { Session } from "@/runtime/session"
import { MessageV2 } from "@/runtime/session/message-v2"
import { Identifier } from "@/shared/id/id"
import { Agent } from "@/runtime/loop/agent"
import { SessionPrompt } from "@/runtime/session/prompt"
import { defer } from "@/shared/util/defer"
import { PermissionNext } from "@/runtime/tools/guard/permission/next"
import { Provider } from "@/runtime/providers/provider"
import { ModelThinking } from "@/runtime/providers/discovery/model-thinking"
import { TaskEffort } from "@/runtime/primitives/task-name"
import {
  assertOwnedSubagentRun,
  throwIfSubagentMessageFailed,
} from "@/runtime/agents/subagent-run-contract"

export const available: boolean = true

const REVIEWER = "code-reviewer"
/** Reasoning levels from least to most; the deepest one the model takes is picked. */
const SCALE = ["none", "minimal", "low", "medium", "high", "xhigh", "max"]

/** The deepest variant the model supports, or the same model at high effort when it has none. */
async function deepestVariant(providerID: string, modelID: string): Promise<string | undefined> {
  const model = await Provider.getModel(providerID, modelID).catch(() => undefined)
  const levels = model?.variants ? Object.keys(model.variants) : (ModelThinking.of(modelID)?.levels ?? [])
  const ranked = levels
    .map((level) => ({ level, rank: SCALE.indexOf(level.toLowerCase()) }))
    .filter((x) => x.rank !== -1)
    .sort((a, b) => b.rank - a.rank)
  if (ranked.length > 0) return ranked[0].level
  return TaskEffort.variant({ effort: "high", providerID, modelID })
}

function buildPrompt(params: SecondOpinionInput): string {
  const what =
    params.kind === "plan"
      ? "Review this plan before it is built. Look for missing steps, wrong assumptions, risky ordering, files or callers it forgets, and a simpler way."
      : "Review this finished change before it is called done. Start from `git diff`. Check that the evidence really shows what is claimed, and look for bugs, regressions and checks that were skipped."
  return [
    what,
    "You only read. Change nothing.",
    ...(params.question ? [`Check this most: ${params.question}`] : []),
    "",
    params.kind === "plan" ? "<plan>" : "<summary>",
    params.subject,
    params.kind === "plan" ? "</plan>" : "</summary>",
    ...(params.evidence ? ["", "<evidence>", params.evidence, "</evidence>"] : []),
    "",
    "Answer in this form and nothing else:",
    "VERDICT: agree | concerns | disagree",
    "REASONS:",
    "- one short reason per line, the most important first",
  ].join("\n")
}

/** Reads the verdict and reasons out of the reviewer's reply. */
export function parseOpinion(text: string): { verdict?: SecondOpinionVerdict; reasons: string[] } {
  const v = text.match(/^\s*\**VERDICT\**\s*:\s*\**\s*(agree|concerns|disagree)/im)
  const verdict = v ? (v[1].toLowerCase() as SecondOpinionVerdict) : undefined
  const after = text.split(/^\s*\**REASONS\**\s*:/im)[1] ?? text
  const reasons = after
    .split("\n")
    .map((l) => l.replace(/^\s*(?:[-*]|\d+[.)])\s+/, "").trim())
    .filter((l) => l && !/^\**VERDICT/i.test(l))
    .slice(0, 8)
  return { verdict, reasons }
}

export const SecondOpinionTool = Tool.define<typeof SecondOpinionInput, SecondOpinionMetadata>(SECOND_OPINION_TOOL_ID, {
  description: DESCRIPTION,
  parameters: SecondOpinionInput,
  async execute(params, ctx) {
    await ctx.ask({
      permission: "task",
      patterns: [REVIEWER],
      always: ["*"],
      metadata: { description: "Second opinion", subagent_type: REVIEWER },
    })
    const agent = await Agent.getSubagent(REVIEWER)
    if (!agent) throw new Error(`The ${REVIEWER} agent is not available.`)

    const msg = await MessageV2.get({ sessionID: ctx.sessionID, messageID: ctx.messageID })
    if (msg.info.role !== "assistant") throw new Error("Not an assistant message")
    const model = { modelID: msg.info.modelID, providerID: msg.info.providerID }
    const variant = await deepestVariant(model.providerID, model.modelID)
    const tier = variant ? `${model.modelID} @ ${variant}` : model.modelID

    // Read-only: no write tools, no todos, no further subagents.
    const denied = ["edit", "write", "apply_patch", "todowrite", "todoread", "task", "second_opinion"]
    const session = await Session.create({
      parentID: ctx.sessionID,
      agentID: agent.name,
      title: `Second opinion on a ${params.kind} (@${agent.name} subagent)`,
      permission: denied.map((permission) => ({ permission, pattern: "*", action: "deny" as const })),
    })
    await PermissionNext.setMode(session.id, await PermissionNext.getMode(ctx.sessionID))
    assertOwnedSubagentRun(
      { id: session.id, parentID: session.parentID, profile: session.agentID, title: session.title },
      { parentSessionID: ctx.sessionID, profile: agent.name },
    )

    ctx.metadata({ title: "Second opinion", metadata: { ok: true, kind: params.kind, tier } })
    const cancel = () => SessionPrompt.cancel(session.id)
    ctx.abort.addEventListener("abort", cancel)
    using _ = defer(() => ctx.abort.removeEventListener("abort", cancel))

    const result = await SessionPrompt.prompt({
      messageID: Identifier.ascending("message"),
      sessionID: session.id,
      model,
      ...(variant ? { variant } : {}),
      agent: agent.name,
      tools: Object.fromEntries(denied.map((t) => [t, false])),
      parts: await SessionPrompt.resolvePromptParts(buildPrompt(params)),
    })
    throwIfSubagentMessageFailed(result)
    const text = result.parts.findLast((x) => x.type === "text")?.text ?? ""
    const { verdict, reasons } = parseOpinion(text)

    return {
      title: verdict ? `Second opinion: ${verdict}` : "Second opinion",
      output: [
        `verdict: ${verdict ?? "unclear"}`,
        ...reasons.map((r) => `- ${r}`),
        "",
        "This is advice, not a gate. If you disagree, tell the person both views.",
      ].join("\n"),
      metadata: { ok: verdict !== undefined, kind: params.kind, verdict, reasons, tier },
    }
  },
})
