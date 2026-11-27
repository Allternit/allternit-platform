/**
 * knowledge_flag: report missing, wrong or stale knowledge to the curator.
 * POST /api/v1/knowledge/flags with the session and message ids. Best effort:
 * signed out or an unreachable API gives a plain answer, never a throw.
 */
import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { PlatformApiError, platformRequest } from "@/runtime/bots/platform-api"
import DESCRIPTION from "@/runtime/tools/builtins/knowledge-flag.txt"

export const FLAG_KINDS = ["missing", "wrong", "stale"] as const
export const FLAG_TARGETS = ["skill", "domain", "data_access"] as const

export function flagBody(
  params: { kind: string; target_kind: string; target_ref?: string; evidence: string; suggested_fix?: string },
  ids: { sessionID?: string; messageID?: string; agent?: string },
) {
  return {
    kind: params.kind,
    target_kind: params.target_kind,
    target_ref: params.target_ref?.trim() || undefined,
    evidence: params.evidence.trim(),
    suggested_fix: params.suggested_fix?.trim() || undefined,
    session_id: ids.sessionID,
    turn_id: ids.messageID,
    agent_id: ids.agent,
  }
}

export const KnowledgeFlagTool = Tool.define("knowledge_flag", {
  description: DESCRIPTION,
  parameters: z.object({
    kind: z.enum(FLAG_KINDS).describe("missing, wrong or stale"),
    target_kind: z.enum(FLAG_TARGETS).describe("skill, domain or data_access"),
    target_ref: z.string().optional().describe("Skill name or knowledge file this is about, if known"),
    evidence: z.string().min(1).describe("What you saw, in plain words. No secrets or personal data."),
    suggested_fix: z.string().optional().describe("The correct fact or step, if you know it"),
  }),
  async execute(params, ctx) {
    const body = flagBody(params, { sessionID: ctx.sessionID, messageID: ctx.messageID, agent: ctx.agent })
    try {
      await platformRequest("POST", "/api/v1/knowledge/flags", body, { timeoutMs: 8_000 })
      return { title: "Knowledge flagged", output: "Flagged. The curator will look at it.", metadata: { ok: true } }
    } catch (error) {
      const why =
        error instanceof PlatformApiError
          ? `the server answered ${error.status}`
          : "the Allternit API is not reachable or you are signed out"
      return {
        title: "Knowledge flag not sent",
        output: `Could not send the flag: ${why}. Nothing was saved.`,
        metadata: { ok: false },
      }
    }
  },
})
