import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { PaneRender } from "@/runtime/integrations/pane-render"
import DESCRIPTION from "@/runtime/tools/builtins/media-generate.txt"

const MAX_CODE = 400_000
const MIN_SIDE = 64
const MAX_SIDE = 4096

export const MediaGenerateTool = Tool.define("media_generate", {
  description: DESCRIPTION,
  parameters: z.object({
    kind: z.enum(["image", "video"]).describe("What to make"),
    prompt: z.string().describe("What the user asked for, in their words"),
    title: z.string().optional().describe("Short name for the output; reuse it to replace a previous version"),
    lane: z
      .enum(["native", "cloud", "subscription", "own_key"])
      .optional()
      .describe("How to make it; native = you write the image as code (the only lane in this build)"),
    format: PaneRender.Format.optional().describe("Native lane: svg, html or canvas"),
    code: z.string().optional().describe("Native lane: the SVG, HTML document, or canvas function body"),
    width: z.number().int().optional().describe("Pixels, 64–4096 (default 1024)"),
    height: z.number().int().optional().describe("Pixels, 64–4096 (default 1024)"),
  }),
  async execute(params, ctx) {
    const title = (params.title ?? params.prompt).trim().slice(0, 80) || "Image"
    if (params.kind === "video") {
      return {
        title: "Video: not available yet",
        output:
          "Video generation isn't available in this build yet (native video is the next release). Tell the user, and offer a still image or a storyboard instead.",
        metadata: { ok: false, lane: null },
      }
    }
    const lane = params.lane ?? (params.code ? "native" : undefined)
    if (lane !== "native") {
      return {
        title: "Image: choose how to make it",
        output: [
          lane
            ? `The ${lane} lane isn't available in this build yet.`
            : "No lane chosen and no code given.",
          "Available now: native — write the image as svg, html or canvas code and call media_generate again with lane \"native\", format and code.",
          "If the request needs a photo-realistic image, ask the user in the chat how to proceed (e.g. a stylized native version).",
        ].join("\n"),
        metadata: { ok: false, lane: lane ?? null },
      }
    }
    if (!params.format || !params.code?.trim()) {
      throw new Error("The native lane needs format (svg, html or canvas) and code")
    }
    if (params.code.length > MAX_CODE) {
      throw new Error(`The code is ${params.code.length} characters; keep it under ${MAX_CODE}`)
    }
    const clamp = (n: number | undefined) => Math.min(MAX_SIDE, Math.max(MIN_SIDE, Math.round(n ?? 1024)))
    const width = clamp(params.width)
    const height = clamp(params.height)

    const result = await PaneRender.request(
      {
        sessionID: ctx.sessionID,
        kind: "image",
        format: params.format,
        code: params.code,
        width,
        height,
        title,
        toolCall: ctx.callID ? { messageID: ctx.messageID, callID: ctx.callID } : undefined,
      },
      { abort: ctx.abort },
    )

    if (!result.ok || !result.image) {
      return {
        title: `Image: ${title} — render failed`,
        output: `The render failed: ${result.error ?? "no image came back"}. Fix the code and call again with the same title.`,
        metadata: { ok: false, lane: "native", format: params.format, width, height },
      }
    }
    return {
      title: `Image: ${title}`,
      output: `Rendered "${title}" (${width}×${height}, ${params.format}). It's in the session's Outputs. The PNG is attached: check it against the request, and if anything is off, call again with the same title and fixed code.`,
      metadata: { ok: true, lane: "native", format: params.format, width, height },
      attachments: [{ type: "file" as const, mime: "image/png", url: result.image, filename: `${title}.png` }],
    }
  },
})
