import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { SkillGrowth } from "@/runtime/skills/growth"
import { activateSkillProposal } from "@/runtime/skills/growth-person"

/**
 * The record, plus the proposals approved (here or in a review screen) that
 * still wait for `skill_activate`. Server approvals are pulled by every
 * skill-growth call; they are never activated without that tool's ask.
 */
async function output(record: SkillGrowth.Record) {
  const waiting = (await SkillGrowth.awaitingActivation()).filter((item) => item.id !== record.id)
  const sync =
    record.sync === "pending"
      ? "\nNot on the server yet (offline or signed out); it syncs on the next skill-growth call."
      : record.sync === "failed"
        ? `\nThe server refused this proposal: ${record.sync_error ?? "unknown error"}. It stays on this device only.`
        : ""
  const notes = waiting.length
    ? `\n\nApproved and waiting for skill_activate (asks the person first):\n${waiting
        .map((item) => `- ${item.id} ${item.name} v${item.version} (${item.origin === "curator" ? "curator" : "skill growth"}, approved on ${item.approved_on === "server" ? "the server" : "this device"})`)
        .join("\n")}`
    : ""
  return `${JSON.stringify(record, null, 2)}${sync}${notes}`
}

export const SkillProposeTool = Tool.define("skill_propose", {
  description:
    "Create an inert, versioned skill proposal. This does not activate the skill. The proposal also goes to the person's review list (web, phone, /knowledge).",
  parameters: z.object({
    name: z.string(),
    description: z.string(),
    content: z.string(),
    scope: z.enum(["user", "project"]).default("project"),
  }),
  async execute(params, ctx) {
    await ctx.ask({ permission: "skill_manage", patterns: [`propose:${params.scope}:${params.name}`], always: [], metadata: {} })
    const record = await SkillGrowth.propose(params)
    return {
      title: `Proposed ${record.name}`,
      metadata: { id: record.id, status: record.status, server_id: record.server_id, sync: record.sync },
      output: await output(record),
    }
  },
})

export const SkillEvaluateTool = Tool.define("skill_evaluate", {
  description: "Attach a reproducible evaluation score and report to an inert skill proposal.",
  parameters: z.object({ proposal_id: z.string(), score: z.number().min(0).max(1), report: z.string() }),
  async execute(params) {
    const record = await SkillGrowth.evaluate(params.proposal_id, { score: params.score, report: params.report })
    return {
      title: `Evaluated ${record.name}`,
      metadata: { id: record.id, status: record.status, score: record.score, sync: record.sync },
      output: await output(record),
    }
  },
})

export const SkillDecideTool = Tool.define("skill_decide", {
  description: "Approve or reject an evaluated skill proposal. Approval still does not activate it.",
  parameters: z.object({ proposal_id: z.string(), decision: z.enum(["approve", "reject"]) }),
  async execute(params, ctx) {
    await ctx.ask({ permission: "skill_manage", patterns: [`${params.decision}:${params.proposal_id}`], always: [], metadata: {} })
    const record = await SkillGrowth.decide(params.proposal_id, params.decision)
    return {
      title: `${params.decision}d ${record.name}`,
      metadata: { id: record.id, status: record.status, sync: record.sync },
      output: await output(record),
    }
  },
})

export const SkillActivateTool = Tool.define("skill_activate", {
  description:
    "Activate an approved skill version, preserving the previous target for rollback. Also activates proposals the person approved in a review screen, including curator edits; proposal_id may be the local id or the server id (kprop_…).",
  parameters: z.object({ proposal_id: z.string() }),
  async execute(params, ctx) {
    const record = await activateSkillProposal(params.proposal_id, (request) => ctx.ask(request))
    return {
      title: `Activated ${record.name}`,
      metadata: { id: record.id, status: record.status, target: record.target, sync: record.sync },
      output: await output(record),
    }
  },
})

export const SkillRollbackTool = Tool.define("skill_rollback", {
  description: "Roll back an activated skill if its target has not been modified since activation.",
  parameters: z.object({ proposal_id: z.string() }),
  async execute(params, ctx) {
    await ctx.ask({ permission: "skill_manage", patterns: [`rollback:${params.proposal_id}`], always: [], metadata: {} })
    const record = await SkillGrowth.rollback(params.proposal_id)
    return {
      title: `Rolled back ${record.name}`,
      metadata: { id: record.id, status: record.status, sync: record.sync },
      output: await output(record),
    }
  },
})
