import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { PaneRender } from "@/runtime/integrations/pane-render"
import DESCRIPTION from "@/runtime/tools/builtins/media-generate.txt"
import { availableProviders, fabricCapabilities } from "@/runtime/providers/fabric/tasks"
import { attachmentsFor, describeOutcome, providerName, runSubscriptionTask } from "@/runtime/tools/builtins/subscription"

interface Meta {
  ok: boolean
  lane: string | null
  provider?: string
  taskID?: string
  artifacts?: Array<{ id: string; mime: string; sha256: string }>
  format?: string
  width?: number
  height?: number
  video?: PaneRender.Video
}
const meta = (m: Meta): Meta => m

const MAX_CODE = 400_000
const MIN_SIDE = 64
const MAX_SIDE = 4096
const MAX_VIDEO_PIXELS = 3840 * 2160
const MAX_DURATION = 60
const FPS = [24, 25, 30, 60] as const

/** Render budget for a video: generous per drawn frame, capped at 15 minutes. */
export function videoTimeoutMs(frames: number, samples: number): number {
  return Math.min(15 * 60_000, 90_000 + frames * samples * 40)
}

interface VideoParams {
  format: PaneRender.Format
  code: string
  width?: number
  height?: number
  duration?: number
  fps?: number
  motionBlur?: number
  audio?: string
}

async function renderVideo(params: VideoParams, title: string, ctx: Tool.Context) {
  if (params.format !== "canvas") {
    throw new Error('A native video is a canvas scene: use format "canvas" with the body of draw(ctx, t, width, height)')
  }
  // H.264 needs even sides; keep the frame under 4K.
  const even = (n: number) => Math.max(MIN_SIDE, Math.round(n / 2) * 2)
  let width = even(Math.min(MAX_SIDE, params.width ?? 1920))
  let height = even(Math.min(MAX_SIDE, params.height ?? 1080))
  if (width * height > MAX_VIDEO_PIXELS) {
    const k = Math.sqrt(MAX_VIDEO_PIXELS / (width * height))
    width = even(width * k)
    height = even(height * k)
  }
  const duration = Math.min(MAX_DURATION, Math.max(1, Math.round((params.duration ?? 6) * 10) / 10))
  const fps = FPS.includes(params.fps as (typeof FPS)[number]) ? (params.fps as number) : 30
  const motionBlur = Math.min(8, Math.max(1, Math.round(params.motionBlur ?? 4)))
  const frames = Math.round(duration * fps)

  const result = await PaneRender.request(
    {
      sessionID: ctx.sessionID,
      kind: "video",
      format: "canvas",
      code: params.code,
      width,
      height,
      duration,
      fps,
      motionBlur,
      audio: params.audio?.trim() ? params.audio : undefined,
      title,
      toolCall: ctx.callID ? { messageID: ctx.messageID, callID: ctx.callID } : undefined,
    },
    { abort: ctx.abort, timeoutMs: videoTimeoutMs(frames, motionBlur) },
  )

  if (!result.ok || !result.video) {
    return {
      title: `Video: ${title} — render failed`,
      output: `The render failed: ${result.error ?? "no video came back"}. Fix the code and call again with the same title.`,
      metadata: meta({ ok: false, lane: "native", format: "canvas", width, height }),
    }
  }
  const sound = result.video.audio ? ", with sound" : ""
  return {
    title: `Video: ${title}`,
    output: `Rendered "${title}" (${width}×${height}, ${duration}s at ${fps}fps${sound}). Video: ${result.video.url}. It's in the session's Outputs. The contact sheet (six stills across the video, timestamped) is attached: check motion, framing and text against the request, and if anything is off, call again with the same title and fixed code.`,
    metadata: meta({ ok: true, lane: "native", format: "canvas", width, height, video: result.video }),
    ...(result.image
      ? { attachments: [{ type: "file" as const, mime: "image/png", url: result.image, filename: `${title} contact sheet.png` }] }
      : {}),
  }
}

/**
 * Subscription lane: `image.generate` on one of the user's connected
 * subscriptions, run on their Sessions computer. The user confirms each
 * image (D16); the image comes back checksum-verified.
 */
async function subscriptionImage(prompt: string, title: string, provider: string | undefined, ctx: Tool.Context) {
  const live = availableProviders(await fabricCapabilities(), "image.generate")
  const chosen = provider ?? live[0]
  if (!chosen || !live.includes(chosen)) {
    return {
      title: "Image: subscription lane not available",
      output: chosen && live.length
        ? `${providerName(chosen)} cannot make images right now. Available: ${live.map(providerName).join(", ")}. Nothing was sent.`
        : "No connected subscription can make images right now (none set up, or the Sessions computer is not reachable). Nothing was sent. Offer the native lane or ask the user how to proceed.",
      metadata: meta({ ok: false, lane: "subscription" }),
    }
  }
  const result = await runSubscriptionTask(ctx, {
    capability: "image.generate",
    provider: chosen,
    prompt,
    title,
    summary: `Make an image with your ${providerName(chosen)} subscription: "${title}"`,
  })
  const images = result.files.filter((f) => f.mime.startsWith("image/"))
  const outcome = describeOutcome({ ...result, files: images }, chosen, "image")
  const ok = outcome.ok && images.length > 0
  const note = result.task.result?.text?.trim()
  return {
    title: `Image: ${title}${ok ? "" : " — not finished"}`,
    output: ok
      ? `${providerName(chosen)} made "${title}". It's in the session's Outputs. The image is attached: check it against the request.${note ? `\n\n${note}` : ""}`
      : outcome.ok
        ? `${providerName(chosen)} finished but returned no image.`
        : outcome.text,
    metadata: meta({
      ok,
      lane: "subscription",
      provider: chosen,
      taskID: result.task.task_id,
      artifacts: images.map((f) => ({ id: f.artifactID, mime: f.mime, sha256: f.sha256 })),
    }),
    attachments: attachmentsFor(images, title),
  }
}

export const MediaGenerateTool = Tool.define("media_generate", {
  description: DESCRIPTION,
  parameters: z.object({
    kind: z.enum(["image", "video"]).describe("What to make"),
    prompt: z.string().describe("What the user asked for, in their words"),
    title: z.string().optional().describe("Short name for the output; reuse it to replace a previous version"),
    lane: z
      .enum(["native", "cloud", "subscription", "own_key"])
      .optional()
      .describe("How to make it; native = you write it as code; subscription = a connected subscription makes the image (the user confirms each one)"),
    provider: z
      .string()
      .optional()
      .describe("Subscription lane: which subscription (chatgpt, claude, kimi); default the first that can"),
    format: PaneRender.Format.optional().describe("Native lane: svg, html or canvas (video: canvas)"),
    code: z
      .string()
      .optional()
      .describe("Native lane: the SVG, HTML document, or canvas function body; for video, the body of draw(ctx, t, width, height)"),
    width: z.number().int().optional().describe("Pixels (image 64–4096, default 1024; video default 1920)"),
    height: z.number().int().optional().describe("Pixels (image 64–4096, default 1024; video default 1080)"),
    duration: z.number().optional().describe("Video: seconds, 1–60 (default 6)"),
    fps: z.number().int().optional().describe("Video: 24, 25, 30 or 60 (default 30)"),
    motionBlur: z.number().int().optional().describe("Video: sub-frame samples per frame for motion blur, 1–8 (default 4; 1 = off)"),
    audio: z
      .string()
      .optional()
      .describe("Video: optional soundtrack, the body of score(ctx, duration) on an OfflineAudioContext"),
  }),
  async execute(params, ctx) {
    const noun = params.kind === "video" ? "Video" : "Image"
    const title = (params.title ?? params.prompt).trim().slice(0, 80) || noun
    const lane = params.lane ?? (params.code ? "native" : undefined)
    if (lane === "subscription" && params.kind === "image") {
      return subscriptionImage(params.prompt, title, params.provider, ctx)
    }
    if (lane !== "native") {
      return {
        title: `${noun}: choose how to make it`,
        output: [
          lane
            ? `The ${lane} lane isn't available in this build yet.`
            : "No lane chosen and no code given.",
          params.kind === "video"
            ? "Available now: native — write the video as a canvas scene (the body of draw(ctx, t, width, height)) and call media_generate again with lane \"native\", format \"canvas\" and code."
            : "Available now: native — write the image as svg, html or canvas code and call media_generate again with lane \"native\", format and code.",
          `If the request needs photo-realistic ${params.kind === "video" ? "footage" : "imagery"}, ask the user in the chat how to proceed (e.g. a stylized native version).`,
        ].join("\n"),
        metadata: meta({ ok: false, lane: lane ?? null }),
      }
    }
    if (params.kind === "video" && !params.format) params.format = "canvas"
    if (!params.format || !params.code?.trim()) {
      throw new Error("The native lane needs format (svg, html or canvas) and code")
    }
    if (params.code.length + (params.audio?.length ?? 0) > MAX_CODE) {
      throw new Error(`The code is ${params.code.length} characters; keep it under ${MAX_CODE}`)
    }
    if (params.kind === "video") return renderVideo(params as VideoParams, title, ctx)
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
        metadata: meta({ ok: false, lane: "native", format: params.format, width, height }),
      }
    }
    return {
      title: `Image: ${title}`,
      output: `Rendered "${title}" (${width}×${height}, ${params.format}). It's in the session's Outputs. The PNG is attached: check it against the request, and if anything is off, call again with the same title and fixed code.`,
      metadata: meta({ ok: true, lane: "native", format: params.format, width, height }),
      attachments: [{ type: "file" as const, mime: "image/png", url: result.image, filename: `${title}.png` }],
    }
  },
})
