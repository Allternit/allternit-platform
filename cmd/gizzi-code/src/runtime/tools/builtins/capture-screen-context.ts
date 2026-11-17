import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { ScreenContext } from "@/runtime/integrations/screen-context"
import DESCRIPTION from "@/runtime/tools/builtins/capture-screen-context.txt"

const MAX_TEXT = 20_000
const SOURCE = "capture_screen_context"

function imageMime(dataUrl: string): string | undefined {
  const match = /^data:(image\/(?:png|jpeg|webp));base64,/.exec(dataUrl)
  return match?.[1]
}

/** One shape for both returns: gizzi compiles without strict null checks, so
 * two differently shaped object literals would not unify. */
type CaptureMetadata = { ok: boolean; kind?: "app_view" | "window"; app?: string; title?: string; image?: boolean }

export const CaptureScreenContextTool = Tool.define("capture_screen_context", {
  description: DESCRIPTION,
  parameters: z.object({
    reason: z
      .string()
      .min(1)
      .max(200)
      .describe("Why you need to see the screen, in a few words the user will see (e.g. 'to see which chart you mean')"),
  }),
  async execute(params, ctx) {
    // Ask once per session: the first capture asks, later ones reuse that yes.
    await ScreenContext.ensureAllowed(ctx.sessionID, () =>
      ctx.ask({
        permission: "screen_context",
        patterns: ["*"],
        always: ["*"],
        metadata: { reason: params.reason },
      }),
    )

    const result = await ScreenContext.request(
      {
        sessionID: ctx.sessionID,
        reason: params.reason,
        tool: ctx.callID ? { messageID: ctx.messageID, callID: ctx.callID } : undefined,
      },
      { abort: ctx.abort },
    )

    if (result.ok === false) {
      return {
        title: "Screen context: no capture",
        output: result.error,
        metadata: { ok: false } as CaptureMetadata,
      }
    }

    const { capture } = result
    const text =
      capture.text.length > MAX_TEXT ? `${capture.text.slice(0, MAX_TEXT)}\n…(truncated)` : capture.text
    const header = [
      `Kind: ${capture.kind === "window" ? "window" : "app view"}`,
      `App: ${capture.app}`,
      capture.title && `Title: ${capture.title}`,
    ]
      .filter(Boolean)
      .join("\n")
    const mime = capture.image ? imageMime(capture.image) : undefined
    return {
      title: `Screen context: ${capture.app}${capture.title ? ` — ${capture.title}` : ""}`,
      output: ScreenContext.spotlight(`${header}\n\n${text || "(no readable text)"}`, SOURCE),
      metadata: { ok: true, kind: capture.kind, app: capture.app, title: capture.title, image: !!mime } as CaptureMetadata,
      ...(capture.image && mime
        ? { attachments: [{ type: "file" as const, mime, url: capture.image, filename: "screen.png" }] }
        : {}),
    }
  },
})
