/**
 * glance: the agent's one-line status for this session's sidebar row.
 *
 * Shown in place of the last-message preview on the session's row (desktop
 * rail, phone drawer, bot threads, session overview cards), so the user can
 * follow long-running work without opening it. Stored on the session
 * (Session.setGlance); an empty text clears it.
 */
import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { Session } from "@/runtime/session"

const DESCRIPTION = `Set the one line the user sees for this session in their sidebar.

Use it while you track something that changes over time (PRs, a deploy, a
spend, a queue, a dashboard you made): write the numbers that matter, the
way a status bar would, e.g. "32 green CI · 32 review threads · 4 to close".
Update it when they change. It replaces the last-message preview on the
session's row, so keep it true: when the work is done or the line no longer
applies, call it with an empty text to clear it.

- text: one line, 80 characters or fewer, no sentence, no emoji.
- artifact_id: optional, the artifact this line summarizes.`

export const GlanceTool = Tool.define("glance", {
  description: DESCRIPTION,
  parameters: z.object({
    text: z.string().describe("One line, 80 characters or fewer; empty clears it"),
    artifact_id: z.string().optional().describe("The artifact this line summarizes"),
  }),
  async execute(params, ctx) {
    const text = params.text.replace(/\s+/g, " ").trim().slice(0, 80)
    Session.setGlance({
      sessionID: ctx.sessionID,
      glance: text ? { text, artifactId: params.artifact_id || undefined, at: Date.now() } : null,
    })
    return {
      title: text ? `Glance: ${text}` : "Glance cleared",
      output: text ? `The sidebar row now reads: ${text}` : "The sidebar row shows the last message again.",
      metadata: { text },
    }
  },
})
