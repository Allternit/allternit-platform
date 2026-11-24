import { Bus } from "@/shared/bus"
import { BusEvent } from "@/shared/bus/bus-event"
import { Identifier } from "@/shared/id/id"
import { Instance } from "@/runtime/context/project/instance"
import { Log } from "@/shared/util/log"
import z from "zod/v4"

/**
 * Native media rendering: the app that shows this session renders code the
 * agent wrote (SVG, HTML/CSS, or a canvas script) to a PNG in its own
 * browser engine, in a sandbox, and replies with the image. Videos are a
 * canvas scene drawn per frame and encoded to MP4 by the app, which stores
 * the file and replies with its URL plus a contact sheet. No provider, no
 * key, no install. Same request/reply shape as PaneBrowser and PaneArtifact.
 */
export namespace PaneRender {
  const log = Log.create({ service: "pane-render" })

  export const Format = z.enum(["svg", "html", "canvas"])
  export type Format = z.infer<typeof Format>

  export const Request = z.object({
    id: Identifier.schema("question"),
    sessionID: Identifier.schema("session"),
    /** check: artifact_check — the app renders an artifact (code = its body) at each width and reports what it saw. */
    kind: z.enum(["image", "video", "check"]),
    format: Format,
    code: z.string(),
    width: z.number().int(),
    height: z.number().int(),
    title: z.string(),
    /** Video: seconds, frames per second, sub-frame samples, and optional score code. */
    duration: z.number().optional(),
    fps: z.number().int().optional(),
    motionBlur: z.number().int().optional(),
    audio: z.string().optional(),
    /** Check: the artifact's kind and body format, the widths to render, and a reference picture (data URL). */
    artifactKind: z.string().optional(),
    bodyFormat: z.string().optional(),
    widths: z.array(z.number().int()).optional(),
    reference: z.string().optional(),
    /** When it was asked (ms). The app skips requests older than timeoutMs. */
    time: z.number(),
    /** How long gizzi waits for the reply (ms). */
    timeoutMs: z.number(),
    toolCall: z.object({ messageID: z.string(), callID: z.string() }).optional(),
  })
  export type Request = z.infer<typeof Request>

  export const Video = z.object({
    /** API path of the stored MP4. */
    url: z.string(),
    mime: z.string(),
    bytes: z.number(),
    duration: z.number(),
    fps: z.number(),
    codec: z.string().optional(),
    audio: z.boolean().optional(),
  })
  export type Video = z.infer<typeof Video>

  /** What artifact_check saw at one width. */
  export const CheckWidth = z.object({
    width: z.number(),
    height: z.number(),
    /** JPEG data URL of the first screen. */
    image: z.string().optional(),
    blank: z.boolean(),
    /** px wider than the window (sideways scroll). */
    overflowX: z.number(),
    errors: z.array(z.string()),
    /** 0–1 likeness to the reference, on the width compared with it. */
    similarity: z.number().optional(),
    mismatch: z.array(z.string()).optional(),
  })
  export type CheckWidth = z.infer<typeof CheckWidth>

  export const Result = z.object({
    ok: z.boolean(),
    /** PNG data URL of the render (for a video: its contact sheet). */
    image: z.string().optional(),
    video: Video.optional(),
    checks: z.array(CheckWidth).optional(),
    width: z.number().optional(),
    height: z.number().optional(),
    error: z.string().optional(),
  })
  export type Result = z.infer<typeof Result>

  /**
   * Live progress of one generation (picture or motion). The single shape every
   * surface mirrors: gizzi-code (this file), ai (TS), iOS (Swift), Android (Kotlin).
   * `media_generate` puts it in tool metadata as `progress_detail`, beside
   * `progress` (a short label that is always the current one).
   * Fields that are unknown are absent, never zero or a guess.
   */
  export const ProgressStage = z.enum(["queued", "drawing", "rendering", "encoding", "saving", "downloading"])
  export type ProgressStage = z.infer<typeof ProgressStage>

  export const ProgressDetail = z.object({
    lane: z.enum(["picture", "motion"]),
    stage: ProgressStage,
    /** What is happening, in plain words ("Drawing frame 12 of 180"). */
    label: z.string().max(200),
    /** Motion: the frame being drawn (1-based) and the total. */
    frame: z.number().int().min(0).optional(),
    frames: z.number().int().min(1).optional(),
    /** 0 to 1, of the current stage. */
    fraction: z.number().min(0).max(1).optional(),
    /** URL or data URL of a thumbnail, 320px on the long side or smaller. */
    preview: z
      .string()
      .max(400_000)
      .refine((v) => /^(https?:\/\/|data:image\/|\/)/.test(v), "preview must be an http(s) URL, a data:image URL or an API path")
      .optional(),
    /** Downloading: real bytes so far and in all (absent when the size is unknown), and seconds left. */
    bytes: z.number().int().min(0).optional(),
    total_bytes: z.number().int().min(0).optional(),
    eta_s: z.number().min(0).optional(),
  })
  export type ProgressDetail = z.infer<typeof ProgressDetail>

  export const Event = {
    Requested: BusEvent.define("pane_render.requested", Request),
    Progress: BusEvent.define(
      "pane_render.progress",
      z.object({ sessionID: z.string(), requestID: z.string(), detail: ProgressDetail }),
    ),
    Replied: BusEvent.define(
      "pane_render.replied",
      z.object({ sessionID: z.string(), requestID: z.string(), ok: z.boolean() }),
    ),
  }

  export const TIMEOUT_MS = 60_000

  const state = Instance.state(async () => {
    const pending: Record<string, { info: Request; resolve: (r: Result) => void; timer: ReturnType<typeof setTimeout> }> = {}
    return { pending }
  })

  export async function request(
    input: Omit<Request, "id" | "time" | "timeoutMs">,
    options: {
      timeoutMs?: number
      abort?: AbortSignal
      /** Called for each progress report the app sends for this request. Unsubscribed when the request ends. */
      onProgress?: (detail: ProgressDetail) => void
    } = {},
  ): Promise<Result> {
    const s = await state()
    const id = Identifier.ascending("question")
    const timeoutMs = options.timeoutMs ?? TIMEOUT_MS
    const info: Request = { ...input, id, time: Date.now(), timeoutMs }
    log.info("requesting", { id, kind: input.kind, format: input.format, width: input.width, height: input.height })
    return new Promise<Result>((resolve) => {
      const unsubscribe = options.onProgress
        ? Bus.subscribe(Event.Progress, (event) => {
            if (event.properties.requestID === id) options.onProgress?.(event.properties.detail)
          })
        : undefined
      // Every way out (reply, timeout, abort) goes through here, so the progress subscription never outlives the request.
      let unsubscribed = false
      const done = (result: Result) => {
        if (!unsubscribed) {
          unsubscribed = true
          unsubscribe?.()
        }
        resolve(result)
      }
      const finish = (result: Result) => {
        const entry = s.pending[id]
        if (!entry) return
        clearTimeout(entry.timer)
        delete s.pending[id]
        done(result)
      }
      const timer = setTimeout(
        () =>
          finish({
            ok: false,
            error: `No app rendered the ${input.kind}. The session isn't open in an Allternit app right now, or the render ran past ${Math.round(timeoutMs / 1000)}s.`,
          }),
        timeoutMs,
      )
      s.pending[id] = { info, resolve: done, timer }
      if (options.abort?.aborted) return finish({ ok: false, error: "Cancelled." })
      options.abort?.addEventListener("abort", () => finish({ ok: false, error: "Cancelled." }), { once: true })
      Bus.publish(Event.Requested, info)
    })
  }

  export async function reply(input: { requestID: string; result: Result }): Promise<boolean> {
    const s = await state()
    const entry = s.pending[input.requestID]
    if (!entry) {
      log.warn("reply for unknown request", { requestID: input.requestID })
      return false
    }
    clearTimeout(entry.timer)
    delete s.pending[input.requestID]
    Bus.publish(Event.Replied, { sessionID: entry.info.sessionID, requestID: input.requestID, ok: input.result.ok })
    entry.resolve(input.result)
    return true
  }

  /** The app reports how far a pending request is. False when no such request is waiting. */
  export async function progress(input: { requestID: string; detail: ProgressDetail }): Promise<boolean> {
    const s = await state()
    const entry = s.pending[input.requestID]
    if (!entry) return false
    Bus.publish(Event.Progress, { sessionID: entry.info.sessionID, requestID: input.requestID, detail: input.detail })
    return true
  }

  export async function list(): Promise<Request[]> {
    const s = await state()
    return Object.values(s.pending).map((entry) => entry.info)
  }
}
