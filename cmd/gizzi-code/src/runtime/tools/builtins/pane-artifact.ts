import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { PaneArtifact } from "@/runtime/integrations/pane-artifact"
import DESCRIPTION from "@/runtime/tools/builtins/pane-artifact.txt"

/** describe carries a whole tool catalog (Slides has ~30 tools with schemas). */
const MAX_TEXT = 60_000

export const PaneArtifactTool = Tool.define("pane_artifact", {
  description: DESCRIPTION,
  parameters: z.object({
    action: PaneArtifact.Action.describe("describe, read or call"),
    tool: z.string().optional().describe("For call: the editor tool to run (a name from describe)"),
    input: z
      .record(z.string(), z.unknown())
      .optional()
      .describe("For call: the tool's input, an object matching its schema from describe"),
  }),
  async execute(params, ctx) {
    if (params.action === "call" && !params.tool) {
      throw new Error("The 'call' action needs a tool (a name from describe)")
    }

    const result = await PaneArtifact.request(
      {
        sessionID: ctx.sessionID,
        action: params.action,
        tool: params.action === "call" ? params.tool : undefined,
        input: params.action === "call" ? (params.input ?? {}) : undefined,
        toolCall: ctx.callID ? { messageID: ctx.messageID, callID: ctx.callID } : undefined,
      },
      { abort: ctx.abort },
    )

    const label = params.action === "call" ? params.tool : params.action
    if (!result.ok) {
      return {
        title: `Artifact: ${label} failed`,
        output: result.error ?? result.text ?? "The action failed.",
        metadata: { ok: false, artifact: result.artifact, mutated: false },
      }
    }
    const text =
      result.text && result.text.length > MAX_TEXT ? `${result.text.slice(0, MAX_TEXT)}\n…(truncated)` : result.text
    const mime = result.image?.match(/^data:(image\/[a-z0-9.+-]+);base64,/i)?.[1]
    return {
      title: `${result.artifact ?? "Artifact"}: ${label}`,
      output: [result.artifact && `Open: ${result.artifact}`, text].filter(Boolean).join("\n\n") || "Done.",
      metadata: { ok: true, artifact: result.artifact, mutated: Boolean(result.mutated) },
      // A rendered view of the pane (e.g. a site preview): the model looks at it.
      ...(result.image && mime
        ? { attachments: [{ type: "file" as const, mime, url: result.image, filename: `${label}.${mime === "image/jpeg" ? "jpg" : "png"}` }] }
        : {}),
    }
  },
})
