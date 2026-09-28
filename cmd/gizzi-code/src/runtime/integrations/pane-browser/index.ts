import { Bus } from "@/shared/bus"
import { BusEvent } from "@/shared/bus/bus-event"
import { Identifier } from "@/shared/id/id"
import { Instance } from "@/runtime/context/project/instance"
import { Log } from "@/shared/util/log"
import z from "zod/v4"

/**
 * Browser actions the app runs on the page the user sees (the session's
 * browser pane), instead of a separate automation browser. The tool publishes
 * a request; the app that shows this session's pane runs it and replies.
 * Same request/reply shape as Question.
 */
export namespace PaneBrowser {
  const log = Log.create({ service: "pane-browser" })

  export const Action = z.enum(["read", "goto", "click", "fill", "screenshot"])
  export type Action = z.infer<typeof Action>

  export const Request = z.object({
    id: Identifier.schema("question"),
    sessionID: Identifier.schema("session"),
    action: Action,
    target: z.string().optional(),
    text: z.string().optional(),
    /** When it was asked (ms). The app skips requests older than TIMEOUT_MS,
     * so a replayed event never repeats an action. */
    time: z.number(),
    tool: z
      .object({
        messageID: z.string(),
        callID: z.string(),
      })
      .optional(),
  })
  export type Request = z.infer<typeof Request>

  export const Result = z.object({
    ok: z.boolean(),
    url: z.string().optional(),
    title: z.string().optional(),
    /** Page text (read) or what happened (other actions). */
    text: z.string().optional(),
    /** PNG data URL (screenshot). */
    image: z.string().optional(),
    error: z.string().optional(),
  })
  export type Result = z.infer<typeof Result>

  export const Event = {
    Requested: BusEvent.define("pane_browser.requested", Request),
    Replied: BusEvent.define(
      "pane_browser.replied",
      z.object({ sessionID: z.string(), requestID: z.string(), ok: z.boolean() }),
    ),
  }

  /** No app answered in time: the pane is closed or no app shows this session. */
  export const TIMEOUT_MS = 45_000

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
    log.info("requesting", { id, action: input.action })
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
            error: "The browser pane didn't answer. It may be closed; ask the user to open it, or use the browser tool.",
          }),
        options.timeoutMs ?? TIMEOUT_MS,
      )
      s.pending[id] = { info, resolve, timer }
      // The turn may already be cancelled by the time this runs.
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
