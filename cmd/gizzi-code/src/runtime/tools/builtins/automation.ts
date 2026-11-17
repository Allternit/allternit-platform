/**
 * automation: the user's scheduled tasks (runtime/primitives/automation.ts).
 * Every client gets it (not bot-gated): Gizzi Code, plain chats, workers and
 * bot chats alike. The Ink adapter wraps the same core and shares
 * automation.txt.
 *
 * Permission: one class, `automation`, with the action as the pattern. The
 * defaults (loop/agent.ts) allow `list` and ask for everything else, so an
 * "always" answer covers only the action it was given for.
 *
 * Input is checked before the permission ask, so the user is never asked to
 * approve a request that would be refused. A turn started by a scheduled task
 * can't create one: refused outright, never asked.
 */
import { Tool } from "@/runtime/tools/builtins/tool"
import { Automation } from "@/runtime/primitives/automation"
import { Clock } from "@/runtime/primitives/clock"
import type { AutomationInfo, AutomationRunResult } from "@/runtime/bots/app-bot-routing"
import DESCRIPTION from "@/runtime/tools/builtins/automation.txt"

export type AutomationMetadata = {
  action: Automation.Action
  status: Automation.Outcome["status"]
  /** create, update */
  automation?: AutomationInfo
  /** list */
  automations?: AutomationInfo[]
  /** delete, run_now */
  id?: string
  run?: AutomationRunResult
  /** no_platform: why, and the request that was not sent (the runtime /cron fallback uses it). */
  reason?: Automation.NoPlatformReason
  built?: Automation.Built
  /** create, update: the sentence about approvals the card shows. */
  approval?: string
}

export const AutomationTool = Tool.define("automation", {
  description: DESCRIPTION,
  parameters: Automation.parameters,
  async execute(params, ctx) {
    const lastUser = ctx.messages.findLast((message) => message.info.role === "user")
    const userMetadata = lastUser?.info.role === "user" ? lastUser.info.metadata : undefined
    const userText = lastUser?.parts.find((part) => part.type === "text")
    const built = await Automation.build(params, {
      turnZone: Clock.turnZoneOf(userMetadata),
      fromAutomation: Automation.isAutomationRun(userMetadata, userText?.type === "text" ? userText.text : undefined),
    })
    await ctx.ask({
      permission: "automation",
      patterns: [params.action],
      always: [params.action],
      metadata: {
        action: params.action,
        ...(built.action === "create" || built.action === "update" ? { request: built.request } : {}),
        ...(built.action === "delete" || built.action === "run_now" ? { id: built.request.id } : {}),
      },
    })
    const outcome = await Automation.send({ sessionID: ctx.sessionID, built })
    const metadata: AutomationMetadata = { action: params.action, status: outcome.status }
    if (outcome.status === "no_platform") {
      metadata.reason = outcome.reason
      metadata.built = outcome.built
    } else if (outcome.action === "create" || outcome.action === "update") {
      metadata.automation = outcome.automation
      metadata.approval = Automation.APPROVAL_SENTENCE
    } else if (outcome.action === "list") {
      metadata.automations = outcome.automations
    } else if (outcome.action === "delete") {
      metadata.id = outcome.id
    } else {
      metadata.id = outcome.run.id
      metadata.run = outcome.run
    }
    return {
      title: Automation.title(outcome),
      output: Automation.summary(outcome),
      metadata,
    }
  },
})
