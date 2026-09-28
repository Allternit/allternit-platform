import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { PaneBrowser } from "@/runtime/integrations/pane-browser"
import DESCRIPTION from "@/runtime/tools/builtins/pane-browser.txt"

const MAX_TEXT = 20_000

export const PaneBrowserTool = Tool.define("pane_browser", {
  description: DESCRIPTION,
  parameters: z.object({
    action: PaneBrowser.Action.describe("read, goto, click, fill or screenshot"),
    target: z.string().optional().describe("URL for goto; CSS selector or visible text for click/fill"),
    text: z.string().optional().describe("Text to type for fill"),
  }),
  async execute(params, ctx) {
    if ((params.action === "goto" || params.action === "click" || params.action === "fill") && !params.target) {
      throw new Error(`The '${params.action}' action needs a target`)
    }
    if (params.action === "fill" && params.text === undefined) {
      throw new Error("The 'fill' action needs text")
    }
    if (params.action !== "read" && params.action !== "screenshot") {
      await ctx.ask({
        permission: "browser",
        patterns: [params.target ?? params.action],
        always: ["browser *"],
        metadata: { action: params.action, target: params.target, surface: "pane" },
      })
    }

    const result = await PaneBrowser.request(
      {
        sessionID: ctx.sessionID,
        action: params.action,
        target: params.target,
        text: params.text,
        tool: ctx.callID ? { messageID: ctx.messageID, callID: ctx.callID } : undefined,
      },
      { abort: ctx.abort },
    )

    if (!result.ok) {
      return {
        title: `Browser pane: ${params.action} failed`,
        output: result.error ?? "The action failed.",
        metadata: { ok: false, url: result.url },
      }
    }
    const where = [result.title, result.url].filter(Boolean).join(" — ")
    const text = result.text && result.text.length > MAX_TEXT ? `${result.text.slice(0, MAX_TEXT)}\n…(truncated)` : result.text
    return {
      title: `Browser pane: ${params.action}${params.target ? ` ${params.target}` : ""}`,
      output: [where && `Page: ${where}`, text].filter(Boolean).join("\n\n") || "Done.",
      metadata: { ok: true, url: result.url },
      ...(result.image
        ? { attachments: [{ type: "file" as const, mime: "image/png", url: result.image, filename: "pane.png" }] }
        : {}),
    }
  },
})
