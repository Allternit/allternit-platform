import { Bus } from "@/shared/bus"
import { BusEvent } from "@/shared/bus/bus-event"
import { Identifier } from "@/shared/id/id"
import { Instance } from "@/runtime/context/project/instance"
import { Log } from "@/shared/util/log"
import z from "zod/v4"

/**
 * Work on the artifact open in the session's editor pane (an Allternit
 * Docs/Sheets/Slides document) through that editor's own agent tools — the
 * document the user sees, not a copy. The tool publishes a request; the app
 * that shows this session's pane runs it and replies. Same request/reply
 * shape as PaneBrowser.
 */
export namespace PaneArtifact {
  const log = Log.create({ service: "pane-artifact" })

  export const Action = z.enum(["describe", "read", "call"])
  export type Action = z.infer<typeof Action>

  export const Request = z.object({
    id: Identifier.schema("question"),
    sessionID: Identifier.schema("session"),
    action: Action,
    /** The editor tool to run (call). */
    tool: z.string().optional(),
    /** That tool's input (call). */
    input: z.record(z.string(), z.unknown()).optional(),
    /** When it was asked (ms). The app skips requests older than TIMEOUT_MS,
     * so a replayed event never repeats an edit. */
    time: z.number(),
    toolCall: z
      .object({
        messageID: z.string(),
        callID: z.string(),
      })
      .optional(),
  })
  export type Request = z.infer<typeof Request>

  export const Result = z.object({
    ok: z.boolean(),
    /** What the pane has open, e.g. `Document "Q3 plan"`. */
    artifact: z.string().optional(),
    /** describe: the editor's guide + tools; read: the document; call: the tool's output. */
    text: z.string().optional(),
    /** call: the tool changed the document. */
    mutated: z.boolean().optional(),
    /** A picture of what the pane shows (PNG data URL), e.g. a rendered site preview. */
    image: z.string().startsWith("data:image/").optional(),
    error: z.string().optional(),
  })
  export type Result = z.infer<typeof Result>

  export const Event = {
    Requested: BusEvent.define("pane_artifact.requested", Request),
    Replied: BusEvent.define(
      "pane_artifact.replied",
      z.object({ sessionID: z.string(), requestID: z.string(), ok: z.boolean() }),
    ),
  }

  /** No app answered in time: no app shows this session. Longer than
   * PaneBrowser's: some editor tools (a whole deck) take a while. */
  export const TIMEOUT_MS = 120_000

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
    log.info("requesting", { id, action: input.action, tool: input.tool })
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
            error: "The artifact editor didn't answer. No app is showing this session; ask the user to open it.",
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
