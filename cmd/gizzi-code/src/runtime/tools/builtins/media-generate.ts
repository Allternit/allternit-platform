import z from "zod/v4"
import { isPersonTurn } from "@/runtime/providers/person-plan/rules"
import { LabelField } from "@/runtime/primitives/gap-types"
import { Tool } from "@/runtime/tools/builtins/tool"
import { PaneRender } from "@/runtime/integrations/pane-render"
import DESCRIPTION from "@/runtime/tools/builtins/media-generate.txt"
import { codexEngine } from "./codex-image"
import { claimImageJob, resolveImageJob, type EngineResult, type ImageEngine } from "./image-engine"
import { GIF_MAX_DIMENSION, GIF_MAX_FRAMES, gifResult } from "@/runtime/integrations/pane-render/gif-result"

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
  engine?: "codex" | "chatgpt_browser" | "native"
  /** The attached picture is also published as a generated file (session/generated-file.ts toolOutputFilePart). */
  outputFile?: boolean
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
    metadata: meta({ ok: true, lane: "native", engine: "native", format: "canvas", width, height, video: result.video, ...(poster && poster.length <= 400_000 ? { poster } : {}) }),
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
    return { title: `Image: ${title} — needs your request`, output: "Pictures only run from a message you send yourself in chat. Nothing was sent.", metadata: meta({ ok: false, lane: "aci", code: "person_required" }) }
  }
  if ((provider && provider !== "chatgpt") || !prompt.trim() || prompt.length > 16_000) {
    return { title: `Image: ${title} — request unavailable`, output: "Pictures need a prompt of 1–16000 characters. Nothing was sent.", metadata: meta({ ok: false, lane: "aci", code: "invalid_request" }) }
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
    output: result.ok ? "The picture engine returned an invalid or oversized image. No image was attached." : (result.error || "No picture came back."),
    metadata: meta({ ok: false, lane: "aci", provider: "chatgpt", code: result.code ?? (result.ok ? "invalid_image" : "unavailable") }),
  }
  return {
    title: `Image: ${title}`, output: `Made "${title}". The picture is attached and in Outputs.`,
    metadata: meta({ ok: true, lane: "aci", provider: "chatgpt", width: image.width, height: image.height }),
    attachments: [{ type: "file" as const, mime: "image/png", url: image.image, filename: `${title}.png` }],
  }
}

const CLAIM_WAIT_MS = 15_000
const ENGINE_LANE = { codex: "codex", chatgpt_browser: "aci" } as const

/** The device's own browser picture engine, built from the browser-image PaneRender job. */
function chatgptBrowserEngine(title: string, ctx: Tool.Context, state: { signInNeeded: boolean }): ImageEngine {
  return {
    id: "chatgpt_browser",
    async run(input): Promise<EngineResult> {
      const result = await PaneRender.request({
        sessionID: ctx.sessionID, kind: "browser-image", engine: "chatgpt-aci", prompt: input.prompt,
        format: "html", code: "", width: 1024, height: 1024, title,
        toolCall: ctx.callID ? { messageID: ctx.messageID, callID: ctx.callID } : undefined,
      }, {
        abort: ctx.abort, timeoutMs: 600_000, claimTimeoutMs: CLAIM_WAIT_MS,
        onProgress: (detail) => {
          input.progress?.(detail.stage === "saving" ? "Saving the picture" : "Making the picture")
        },
      })
      if (!result.ok) {
        if (result.code === "not_started") return { status: "not_started", reason: "no_host" }
        if (result.code === "signed_out" || result.code === "sign_in_needed") {
          state.signInNeeded = true
          return { status: "not_started", reason: "sign_in_needed" }
        }
        // A claimed job that timed out, was stopped or failed: unknown whether the picture was made.
        return { status: "ambiguous" }
      }
      const image = browserImageResult(result)
      if (!image) return { status: "definite", code: "invalid_image", error: "The picture came back in a form that could not be used." }
      return { status: "ok", ...image }
    },
  }
}

/** True when an assistant tool part after this user message already ran an engine job. */
function engineUsedAfter(ctx: Tool.Context, userMessageId: string): boolean {
  const index = ctx.messages.findIndex((m) => m.info.id === userMessageId)
  if (index < 0) return false
  return ctx.messages.slice(index + 1).some((m) =>
    m.parts.some((p: any) => p.type === "tool" && p.tool === "media_generate" && ["codex", "chatgpt_browser"].includes(p.state?.metadata?.engine)),
  )
}

const NO_ENGINE_OUTPUT =
  'No picture engine is available right now. Draw it in code instead: call media_generate again with lane "native", a format (svg, html or canvas) and code, and tell the user it is drawn in code.'

async function pictureJob(title: string, ctx: Tool.Context) {
  const latest = [...ctx.messages].reverse().find((m) => m.info.role === "user")
  const user = latest?.info
  if (!user || user.role !== "user" || user.bot || !isPersonTurn(user.metadata)) {
    return { title: `Image: ${title} — needs your request`, output: "Pictures only come from a message you send yourself in chat. Nothing was sent.", metadata: meta({ ok: false, lane: "aci", code: "person_required" }) }
  }
  const raw = (user.metadata as Record<string, unknown> | undefined)?.image_prompt
  const brief = typeof raw === "string" ? raw : ""
  if (!brief.trim() || brief.length > 16_000) {
    return { title: `Image: ${title} — request unavailable`, output: "The picture request needs 1–16000 characters. Nothing was sent.", metadata: meta({ ok: false, lane: null, code: "invalid_request" }) }
  }
  if (!claimImageJob(user.id) || engineUsedAfter(ctx, user.id)) {
    return { title: `Image: ${title} — already made`, output: "One picture job runs per message. Check the earlier result before asking again; nothing was sent.", metadata: meta({ ok: false, lane: null, code: "already_used" }) }
  }
  const earlierBriefs: string[] = []
  for (const m of ctx.messages) {
    if (m.info.id === user.id) break
    if (m.info.role !== "user" || m.info.bot || !isPersonTurn(m.info.metadata)) continue
    const earlier = (m.info.metadata as Record<string, unknown> | undefined)?.image_prompt
    if (typeof earlier === "string" && earlier.trim()) earlierBriefs.push(earlier)
  }
  const state = { signInNeeded: false }
  const forward = reportProgress(ctx, `Image: ${title}`, {})
  const outcome = await resolveImageJob({
    engines: [codexEngine, chatgptBrowserEngine(title, ctx, state)],
    prompt: brief, earlierBriefs, signal: ctx.abort, now: Date.now,
    progress: (label) => forward({ lane: "picture", stage: label.startsWith("Saving") ? "saving" : "drawing", label }),
  })
  switch (outcome.kind) {
    case "image":
      return {
        title: `Image: ${title}`, output: `Made "${title}". The picture is attached and in Outputs.`,
        metadata: meta({ ok: true, lane: ENGINE_LANE[outcome.engine], engine: outcome.engine, width: outcome.width, height: outcome.height, outputFile: true }),
        attachments: [{ type: "file" as const, mime: "image/png", url: outcome.image, filename: `${title}.png` }],
      }
    case "no_engine":
      return {
        title: `Image: ${title} — no picture engine`, output: NO_ENGINE_OUTPUT,
        metadata: meta({ ok: false, lane: null, code: state.signInNeeded ? "sign_in_needed" : "unavailable" }),
      }
    case "ambiguous":
      return { title: `Image: ${title} — check first`, output: outcome.message, metadata: meta({ ok: false, lane: ENGINE_LANE[outcome.engine], engine: outcome.engine, code: "ambiguous" }) }
    case "error":
      return {
        title: `Image: ${title} — not finished`, output: outcome.message,
        metadata: meta({ ok: false, lane: outcome.engine ? ENGINE_LANE[outcome.engine] : null, ...(outcome.engine ? { engine: outcome.engine } : {}), code: outcome.code }),
      }
  }
}

/** Data URL of an earlier image output whose title matches, or undefined. */
function earlierImage(ctx: Tool.Context, source: string): string | undefined {
  const wanted = source.trim().toLowerCase()
  for (const m of [...ctx.messages].reverse()) {
    for (const p of [...m.parts].reverse() as any[]) {
      if (p.type !== "tool" || p.state?.status !== "completed") continue
      const name = String(p.state.input?.title ?? p.state.title ?? "").replace(/^Image:\s*/, "").trim().toLowerCase()
      if (name !== wanted) continue
      const file = (p.state.attachments ?? []).find((a: any) => typeof a.mime === "string" && a.mime.startsWith("image/") && typeof a.url === "string" && a.url.startsWith("data:image/"))
      if (file) return file.url
    }
  }
}

async function renderGif(params: { code?: string; width?: number; height?: number; duration?: number; fps?: number; source?: string }, title: string, ctx: Tool.Context) {
  const side = (n: number | undefined) => Math.min(GIF_MAX_DIMENSION, Math.max(MIN_SIDE, Math.round(n ?? 480)))
  const width = side(params.width)
  const height = side(params.height)
  const fps = Math.min(15, Math.max(1, Math.round(params.fps ?? 12)))
  const duration = Math.min(8, GIF_MAX_FRAMES / fps, Math.max(0.5, params.duration ?? 3))
  const frames = Math.max(1, Math.round(duration * fps))
  let source: string | undefined
  if (params.source?.trim()) {
    source = earlierImage(ctx, params.source)
    if (!source) {
      return { title: `GIF: ${title} — picture not found`, output: `No earlier picture titled "${params.source}" is in this chat. Check the title, or draw the animation without source.`, metadata: meta({ ok: false, lane: "native", engine: "native", code: "invalid_request" }) }
    }
  }
  const result = await PaneRender.request(
    {
      sessionID: ctx.sessionID, kind: "gif", format: "canvas", code: params.code!, width, height, duration, fps,
      ...(source ? { source } : {}), title,
      toolCall: ctx.callID ? { messageID: ctx.messageID, callID: ctx.callID } : undefined,
    },
    { abort: ctx.abort, timeoutMs: Math.min(5 * 60_000, 60_000 + frames * 500), onProgress: reportProgress(ctx, `GIF: ${title}`, { lane: "native", engine: "native" }) },
  )
  const gif = result.ok ? gifResult(result) : undefined
  if (!gif) {
    return {
      title: `GIF: ${title} — render failed`,
      output: `The GIF render failed: ${result.ok ? "the GIF that came back could not be used" : (result.error ?? "no GIF came back")}. Fix the code and call again with the same title.`,
      metadata: meta({ ok: false, lane: "native", engine: "native", format: "canvas", width, height, code: result.ok ? "invalid_image" : result.code }),
    }
  }
  return {
    title: `GIF: ${title}`,
    output: `Made "${title}" (${gif.width}×${gif.height}, ${gif.frames} frames, loops). The GIF is attached and in Outputs.`,
    metadata: meta({ ok: true, lane: "native", engine: "native", format: "canvas", width: gif.width, height: gif.height, outputFile: true }),
    attachments: [{ type: "file" as const, mime: "image/gif", url: gif.image, filename: `${title}.gif` }],
  }
}

export const MediaGenerateTool = Tool.define("media_generate", {
  description: DESCRIPTION,
  parameters: z.object({
    ...LabelField,
    kind: z.enum(["image", "gif", "video"]).describe("What to make"),
    prompt: z.string().describe("What the user asked for, in their words"),
    title: z.string().optional().describe("Short name for the output; reuse it to replace a previous version"),
    lane: z
      .enum(["native", "aci", "codex", "cloud", "subscription", "own_key"])
      .optional()
      .describe("native = you draw it in code. Leave unset for a picture from the person's Image-mode request: a picture engine is chosen automatically. aci, codex and subscription are old values and are treated as unset."),
    provider: z.string().optional().describe("Ignored."),
    format: PaneRender.Format.optional().describe("Native lane: svg, html or canvas (video and gif: canvas)"),
    code: z
      .string()
      .optional()
      .describe("Native lane: the SVG, HTML document, or canvas function body; for video, the body of draw(ctx, t, width, height)"),
    width: z.number().int().optional().describe("Pixels (image 64–4096, default 1024; video default 1920)"),
    height: z.number().int().optional().describe("Pixels (image 64–4096, default 1024; video default 1080)"),
    duration: z.number().optional().describe("Video: seconds, 1–60 (default 6). GIF: seconds, up to 8 (default 3)"),
    fps: z.number().int().optional().describe("Video: 24, 25, 30 or 60 (default 30). GIF: 1–15 (default 12)"),
    motionBlur: z.number().int().optional().describe("Video: sub-frame samples per frame for motion blur, 1–8 (default 4; 1 = off)"),
    source: z.string().optional().describe("GIF: title of an earlier picture in this chat to animate (pan, zoom, overlay)"),
    audio: z
      .string()
      .optional()
      .describe("Video: optional soundtrack, the body of score(ctx, duration) on an OfflineAudioContext"),
  }),
  async execute(params, ctx) {
    const noun = params.kind === "video" ? "Video" : params.kind === "gif" ? "GIF" : "Image"
    const title = (params.title ?? params.prompt).trim().slice(0, 80) || noun
    const oldLane = ["aci", "codex", "subscription"].includes(params.lane ?? "")
    const lane = params.lane && !oldLane ? params.lane : params.code ? "native" : undefined
    if (params.kind === "image" && !params.code?.trim() && (lane === undefined || lane === "native")) {
      const user = [...ctx.messages].reverse().find((m) => m.info.role === "user")?.info
      const hasBrief = user?.role === "user" && isPersonTurn(user.metadata) && typeof (user.metadata as Record<string, unknown> | undefined)?.image_prompt === "string"
      // Old clients send no image_prompt: keep the device-browser path for them.
      if (user?.role === "user" && isPersonTurn(user.metadata) && !user.bot && !hasBrief && lane === undefined) return aciImage(params.prompt, title, undefined, ctx)
      if (lane === undefined) return pictureJob(title, ctx)
    }
    if (params.kind === "gif") {
      if (!params.code?.trim()) throw new Error("A GIF is drawn in code: give the body of draw(ctx, t, width, height) as code")
      if (params.code.length > MAX_CODE) throw new Error(`The code is ${params.code.length} characters; keep it under ${MAX_CODE}`)
      return renderGif(params, title, ctx)
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
      metadata: meta({ ok: true, lane: "native", engine: "native", format: params.format, width, height }),
      attachments: [{ type: "file" as const, mime: "image/png", url: result.image, filename: `${title}.png` }],
    }
  },
})
