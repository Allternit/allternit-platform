import z from "zod/v4"
import { isPersonTurn } from "@/runtime/providers/person-plan/rules"
import { LabelField } from "@/runtime/primitives/gap-types"
import { Tool } from "@/runtime/tools/builtins/tool"
import { PaneRender } from "@/runtime/integrations/pane-render"
import DESCRIPTION from "@/runtime/tools/builtins/media-generate.txt"
import { generateCodexImage } from "./codex-image"

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
  code?: string
  poster?: string
}
const meta = (m: Meta): Meta => m

/**
 * Turn progress reports into tool metadata updates: at most about one a second,
 * except that a change of stage always goes through at once (the card must not
 * show "drawing" after the render moved on). Reports in between are dropped;
 * the next one carries the newer numbers.
 */
export function progressForwarder(
  emit: (detail: PaneRender.ProgressDetail) => void,
  options: { intervalMs?: number; now?: () => number } = {},
) {
  const intervalMs = options.intervalMs ?? 1000
  const now = options.now ?? Date.now
  let lastAt = -Infinity
  let lastStage: PaneRender.ProgressStage | undefined
  return (detail: PaneRender.ProgressDetail) => {
    const t = now()
    if (detail.stage === lastStage && t - lastAt < intervalMs) return
    lastAt = t
    lastStage = detail.stage
    emit(detail)
  }
}

/** The live metadata a running generation reports: `progress` is the current label, `progress_detail` the full report. */
function reportProgress(ctx: Tool.Context, title: string, extra: Record<string, unknown> = {}) {
  return progressForwarder((detail) =>
    ctx.metadata({
      title: `${title} — ${detail.label}`,
      metadata: { ...extra, progress: detail.label, progress_detail: detail },
    }),
  )
}

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

  let poster: string | undefined
  const forward = reportProgress(ctx, `Video: ${title}`, { lane: "native" })
  const onProgress = (detail: PaneRender.ProgressDetail) => {
    if (detail.preview) poster = detail.preview
    forward(detail)
  }
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
    { abort: ctx.abort, timeoutMs: videoTimeoutMs(frames, motionBlur), onProgress },
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
    metadata: meta({ ok: true, lane: "native", format: "canvas", width, height, video: result.video, ...(poster && poster.length <= 400_000 ? { poster } : {}) }),
    ...(result.image
      ? { attachments: [{ type: "file" as const, mime: "image/png", url: result.image, filename: `${title} contact sheet.png` }] }
      : {}),
  }
}

/** Validate the normalized PNG before attaching untrusted browser output to a chat. */
export function browserImageResult(result: PaneRender.Result): { image: string; width: number; height: number } | undefined {
  if (!result.ok || !result.image?.startsWith("data:image/png;base64,")) return
  const encoded = result.image.slice("data:image/png;base64,".length)
  // A bounded PNG keeps the reply under the API's 16 MiB JSON body limit.
  if (!encoded || encoded.length > 11_184_812 || !/^[A-Za-z0-9+/]*={0,2}$/.test(encoded)) return
  const bytes = Buffer.from(encoded, "base64")
  if (bytes.length > 8 * 1024 * 1024 || bytes.length < 33 || bytes.toString("base64") !== encoded) return
  if (!bytes.subarray(0, 8).equals(Buffer.from([137, 80, 78, 71, 13, 10, 26, 10])) || bytes.toString("ascii", 12, 16) !== "IHDR") return
  if (bytes.readUInt32BE(8) !== 13) return
  let offset = 8
  let hasData = false
  let ended = false
  while (offset + 12 <= bytes.length) {
    const size = bytes.readUInt32BE(offset)
    if (size > bytes.length - offset - 12) return
    const type = bytes.toString("ascii", offset + 4, offset + 8)
    if (type === "IDAT" && size > 0) hasData = true
    offset += size + 12
    if (type === "IEND") {
      if (size !== 0 || offset !== bytes.length) return
      ended = true
      break
    }
  }
  if (!hasData || !ended) return
  const width = bytes.readUInt32BE(16)
  const height = bytes.readUInt32BE(20)
  if (!width || !height || width > 4096 || height > 4096 || width * height > 16_000_000) return
  if ((result.width !== undefined && result.width !== width) || (result.height !== undefined && result.height !== height)) return
  return { image: result.image, width, height }
}

async function aciImage(prompt: string, title: string, provider: string | undefined, ctx: Tool.Context) {
  const latestUser = [...ctx.messages].reverse().find((m) => m.info.role === "user")?.info
  if (!latestUser || latestUser.role !== "user" || latestUser.bot || !isPersonTurn(latestUser.metadata)) {
    return { title: `Image: ${title} — needs your request`, output: "Local ACI images only run from a message you send yourself in chat. Nothing was sent.", metadata: meta({ ok: false, lane: "aci", code: "person_required" }) }
  }
  if ((provider && provider !== "chatgpt") || !prompt.trim() || prompt.length > 16_000) {
    return { title: `Image: ${title} — request unavailable`, output: "Local ACI images need provider chatgpt and a prompt of 1–16000 characters. Nothing was sent.", metadata: meta({ ok: false, lane: "aci", code: "invalid_request" }) }
  }
  // The app presents the exact prompt before a provider send. A replayed
  // sync request never grants that confirmation by itself.
  const result = await PaneRender.request({
    sessionID: ctx.sessionID, kind: "browser-image", engine: "chatgpt-aci", prompt,
    format: "html", code: "", width: 1024, height: 1024, title,
    toolCall: ctx.callID ? { messageID: ctx.messageID, callID: ctx.callID } : undefined,
  }, {
    abort: ctx.abort, timeoutMs: 600_000,
    onProgress: (detail) => ctx.metadata({
      title: `Image: ${title} — ${detail.label}`,
      metadata: { lane: "aci", provider: "chatgpt", progress: detail.label, progress_detail: detail },
    }),
  })
  const image = browserImageResult(result)
  if (!image) return {
    title: `Image: ${title} — not finished`,
    output: result.ok ? "The ACI browser returned an invalid or oversized image. No image was attached." : (result.error || "The local ACI browser did not return an image."),
    metadata: meta({ ok: false, lane: "aci", provider: "chatgpt", code: result.code ?? (result.ok ? "invalid_image" : "unavailable") }),
  }
  return {
    title: `Image: ${title}`, output: `Made "${title}" in this device's ACI browser. The image is attached and available in Outputs.`,
    metadata: meta({ ok: true, lane: "aci", provider: "chatgpt", width: image.width, height: image.height }),
    attachments: [{ type: "file" as const, mime: "image/png", url: image.image, filename: `${title}.png` }],
  }
}

async function codexImage(prompt: string, title: string, ctx: Tool.Context) {
  const user = [...ctx.messages].reverse().find((m) => m.info.role === "user")?.info
  if (!user || user.role !== "user" || user.bot || !isPersonTurn(user.metadata)) {
    return { title: `Image: ${title}`, output: "Codex images need a request you send yourself in chat. Nothing was sent.", metadata: meta({ ok: false, lane: "codex", provider: "codex", code: "person_required" }) }
  }
  // Always-ask permission works on native clients and the TUI as well as web.
  // This is a local CLI job, not a remote Subscription Fabric task.
  await ctx.ask({ permission: "subscription", patterns: ["codex:image.generate"], always: [], metadata: {
    toolName: "media_generate", provider: "codex", prompt,
    summary: `Create this image with Codex CLI on the selected runtime, using its ChatGPT sign-in. This is a separate request from ChatGPT ACI.\n\n${prompt}`,
  } })
  const result = await generateCodexImage(prompt, ctx.abort, reportProgress(ctx, `Image: ${title}`, { lane: "codex", provider: "codex" }))
  const image = browserImageResult(result)
  if (!image) return { title: `Image: ${title} — not finished`, output: result.error ?? "Codex returned an invalid or oversized PNG. No image was attached.", metadata: meta({ ok: false, lane: "codex", provider: "codex", code: result.code ?? "invalid_image" }) }
  return { title: `Image: ${title}`, output: `Made "${title}" with Codex CLI. The image is attached and available in Outputs.`,
    metadata: meta({ ok: true, lane: "codex", provider: "codex", width: image.width, height: image.height }),
    attachments: [{ type: "file" as const, mime: "image/png", url: image.image, filename: `${title}.png` }] }
}

export const MediaGenerateTool = Tool.define("media_generate", {
  description: DESCRIPTION,
  parameters: z.object({
    ...LabelField,
    kind: z.enum(["image", "video"]).describe("What to make"),
    prompt: z.string().describe("What the user asked for, in their words"),
    title: z.string().optional().describe("Short name for the output; reuse it to replace a previous version"),
    lane: z
      .enum(["native", "aci", "codex", "cloud", "subscription", "own_key"])
      .optional()
      .describe("How to make it; aci = ChatGPT image in this device’s ACI browser after prompt confirmation; native = code rendering. subscription is a legacy alias of aci, never a remote route."),
    provider: z
      .string()
      .optional()
      .describe("Image provider: chatgpt (device-local ACI) or codex (Codex CLI on the selected runtime, with confirmation). Codex is also an explicit backup after an ACI failure. Never automatically retry an ambiguous browser submission."),
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
    const lane = params.lane ?? (params.code ? "native" : params.kind === "image" ? "aci" : undefined)
    if (params.kind === "image" && (lane === "codex" || (["aci", "subscription"].includes(lane ?? "") && ["codex", "codex-cli"].includes(params.provider ?? "")))) {
      return codexImage(params.prompt, title, ctx)
    }
    if ((lane === "aci" || lane === "subscription") && params.kind === "image") {
      // The old name remains accepted but never routes back to Sessions.
      return aciImage(params.prompt, title, params.provider, ctx)
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
      { abort: ctx.abort, onProgress: reportProgress(ctx, `Image: ${title}`, { lane: "native" }) },
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
