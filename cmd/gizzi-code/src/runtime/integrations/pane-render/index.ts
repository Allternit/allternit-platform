import { Bus } from "@/shared/bus"
import { BusEvent } from "@/shared/bus/bus-event"
import { Identifier } from "@/shared/id/id"
import { Instance } from "@/runtime/context/project/instance"
import { Log } from "@/shared/util/log"
import z from "zod/v4"

/**
 * Native media rendering: the app that shows this session renders code the
 * agent wrote (SVG, HTML/CSS, or a canvas script) to a PNG in its own
 * browser engine, in a sandbox, and replies with the image. No provider, no
 * key, no install. Same request/reply shape as PaneBrowser and PaneArtifact.
 */
export namespace PaneRender {
  const log = Log.create({ service: "pane-render" })

  export const Format = z.enum(["svg", "html", "canvas"])
  export type Format = z.infer<typeof Format>

  export const Request = z.object({
    id: Identifier.schema("question"),
    sessionID: Identifier.schema("session"),
    kind: z.literal("image"),
    format: Format,
    code: z.string(),
    width: z.number().int(),
    height: z.number().int(),
    title: z.string(),
    /** When it was asked (ms). The app skips requests older than TIMEOUT_MS. */
    time: z.number(),
    toolCall: z.object({ messageID: z.string(), callID: z.string() }).optional(),
  })
  export type Request = z.infer<typeof Request>

  export const Result = z.object({
    ok: z.boolean(),
    /** PNG data URL of the render. */
    image: z.string().optional(),
    width: z.number().optional(),
    height: z.number().optional(),
    error: z.string().optional(),
  })
  export type Result = z.infer<typeof Result>

  export const Event = {
    Requested: BusEvent.define("pane_render.requested", Request),
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
    input: Omit<Request, "id" | "time">,
    options: { timeoutMs?: number; abort?: AbortSignal } = {},
  ): Promise<Result> {
    const s = await state()
    const id = Identifier.ascending("question")
    const info: Request = { ...input, id, time: Date.now() }
    log.info("requesting", { id, format: input.format, width: input.width, height: input.height })
    return new Promise<Result>((resolve) => {
      const finish = (result: Result) => {
        const entry = s.pending[id]
        if (!entry) return
        clearTimeout(entry.timer)
        delete s.pending[id]
        resolve(result)
      }
      const timer = setTimeout(
        () =>
          finish({
            ok: false,
            error: "No app rendered the image. The session isn't open in an Allternit app right now.",
          }),
        options.timeoutMs ?? TIMEOUT_MS,
      )
      s.pending[id] = { info, resolve, timer }
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

  export async function list(): Promise<Request[]> {
    const s = await state()
    return Object.values(s.pending).map((entry) => entry.info)
  }
}
