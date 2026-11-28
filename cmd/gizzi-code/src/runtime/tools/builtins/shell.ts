/**
 * shell: the model's one tool for driving the screen (AllternitOS Stage 1, step 13).
 * Contract: handoffs/allternit-os-2026-11-27/CONTRACT.md sections 3.8, 4.1, 7.1, 11, 12.
 *
 * The tool builds a ShellCommandV1 and publishes it on the runtime event bus
 * (`shell.command.requested`); the app that shows this session runs it and
 * replies (`ShellBridge.reply`), the same request/reply shape as PaneBrowser.
 * The app executor is plan step 15. The model never writes to the DOM.
 *
 * Classes: open/close/focus/arrange = ui_layout (allow); start_run/stop_run =
 * run_control (allow); install_app/fork_app = app_install, an ALWAYS_ASK class
 * (next.ts): a person approves every call, at every autonomy level. The model
 * never approves. Protected screens are refused here and again by the shell.
 */
import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { Bus } from "@/shared/bus"
import { BusEvent } from "@/shared/bus/bus-event"
import { Identifier } from "@/shared/id/id"
import { Instance } from "@/runtime/context/project/instance"
import { Log } from "@/shared/util/log"
import DESCRIPTION from "@/runtime/tools/builtins/shell.txt"

/**
 * Copied from spec/Contracts/os/v1/schemas/shell.schema.json, ShellCommandV1
 * 1.0.0 (generated TS: spec/Contracts/os/v1/generated/ts/os-contracts.d.ts).
 * gizzi-code's build cannot import across the repo's spec folder; the contract
 * conformance test keeps the two in step.
 */
export const SHELL_SCHEMA_ID = "allternit.os.ShellCommandV1"
export const SHELL_SCHEMA_VERSION = "1.0.0"

export const ShellAction = z.enum([
  "open",
  "close",
  "focus",
  "arrange",
  "start_run",
  "stop_run",
  "install_app",
  "fork_app",
])
export type ShellAction = z.infer<typeof ShellAction>

const Task = z.union([
  z.object({ prompt: z.string().min(1), profile_id: z.string().min(1) }),
  // A full TaskIRV1 (spec/Contracts/kernel/v1); the kernel validates it.
  z.object({ schema_id: z.literal("allternit.kernel.TaskIRV1") }).passthrough(),
])

const Params = z.object({
  action: z.union([ShellAction, z.literal("get_state")]),
  expected_state_version: z.number().int().min(0).optional(),
  view_id: z.string().min(1).optional(),
  params: z.record(z.string(), z.unknown()).optional(),
  instance_id: z.string().min(1).optional(),
  arrangement: z
    .object({
      layout: z.enum(["single", "split", "grid", "free"]),
      slots: z.array(z.object({ instance_id: z.string().min(1), region: z.string().min(1) })),
    })
    .optional(),
  task: Task.optional(),
  run_id: z.string().min(1).optional(),
  app_source: z
    .object({
      app_id: z.string().min(1),
      source: z.enum(["catalog", "url"]),
      ref: z.string().min(1),
      digest: z.string().regex(/^[0-9a-f]{64}$/, "sha256 hex"),
    })
    .optional(),
  fork_of: z.object({ app_id: z.string().min(1), app_version: z.string().min(1) }).optional(),
})
type Input = z.infer<typeof Params>

export interface ShellCommandV1 {
  schema_id: typeof SHELL_SCHEMA_ID
  schema_version: string
  command_id: string
  action: ShellAction
  expected_state_version?: number
  view_id?: string
  params?: Record<string, unknown>
  instance_id?: string
  arrangement?: { layout: "single" | "split" | "grid" | "free"; slots: { instance_id: string; region: string }[] }
  task?: z.infer<typeof Task>
  run_id?: string
  app_source?: { app_id: string; source: "catalog" | "url"; ref: string; digest: string }
  fork_of?: { app_id: string; app_version: string }
  check_id?: string
}

/** What the app sends back. Matches the shell.schema.json result shapes (CONTRACT 7.1, 11.4). */
export const ShellResult = z.union([
  z.object({
    ok: z.literal(true),
    command_id: z.string(),
    state_version: z.number().int().min(0),
    new_app_id: z.string().optional(),
  }),
  z.object({ ok: z.literal(false), error: z.string(), receipt_id: z.string().optional() }),
  // get_state: a ShellStateV1.
  z.object({ schema_id: z.literal("allternit.os.ShellStateV1") }).passthrough(),
])
export type ShellResult = z.infer<typeof ShellResult>

/** Event bridge: same request/reply shape as PaneBrowser, on the existing runtime bus. */
export namespace ShellBridge {
  const log = Log.create({ service: "shell-tool" })

  export const Request = z.object({
    id: z.string(),
    sessionID: z.string(),
    time: z.number(),
    /** A ShellCommandV1, or `get_state` for the read-only state fetch. */
    kind: z.enum(["command", "get_state"]),
    command: z.custom<ShellCommandV1>().optional(),
  })
  export type Request = z.infer<typeof Request>

  export const Event = {
    Requested: BusEvent.define("shell.command.requested", Request),
    Replied: BusEvent.define(
      "shell.command.replied",
      z.object({ sessionID: z.string(), requestID: z.string(), ok: z.boolean() }),
    ),
  }

  export const TIMEOUT_MS = 30_000

  const state = Instance.state(async () => {
    const pending: Record<string, { info: Request; resolve: (r: ShellResult) => void; timer: ReturnType<typeof setTimeout> }> = {}
    return { pending }
  })

  export async function request(
    input: Omit<Request, "id" | "time">,
    options: { timeoutMs?: number; abort?: AbortSignal } = {},
  ): Promise<ShellResult> {
    const s = await state()
    const id = Identifier.ascending("question")
    const info: Request = { ...input, id, time: Date.now() }
    log.info("requesting", { id, kind: input.kind, action: input.command?.action })
    return new Promise<ShellResult>((resolve) => {
      const finish = (result: ShellResult) => {
        const entry = s.pending[id]
        if (!entry) return
        clearTimeout(entry.timer)
        delete s.pending[id]
        resolve(result)
      }
      const timer = setTimeout(
        () => finish({ ok: false, error: "no_app" }),
        options.timeoutMs ?? TIMEOUT_MS,
      )
      s.pending[id] = { info, resolve, timer }
      if (options.abort?.aborted) return finish({ ok: false, error: "cancelled" })
      options.abort?.addEventListener("abort", () => finish({ ok: false, error: "cancelled" }), { once: true })
      Bus.publish(Event.Requested, info)
    })
  }

  export async function reply(input: { requestID: string; result: ShellResult }): Promise<boolean> {
    const s = await state()
    const entry = s.pending[input.requestID]
    if (!entry) {
      log.warn("reply for unknown request", { requestID: input.requestID })
      return false
    }
    clearTimeout(entry.timer)
    delete s.pending[input.requestID]
    const parsed = ShellResult.safeParse(input.result)
    const result: ShellResult = parsed.success ? parsed.data : { ok: false, error: "bad_request" }
    Bus.publish(Event.Replied, {
      sessionID: entry.info.sessionID,
      requestID: input.requestID,
      ok: !("ok" in result) || result.ok,
    })
    entry.resolve(result)
    return true
  }
}

/** The class ask that gates install_app and fork_app; ALWAYS_ASK in guard/permission/next.ts. */
export const APP_INSTALL_PERMISSION = "app_install"

/** Views the kernel raises itself (ADR-0005, CONTRACT 5.5 and 6): the model can never open them. */
const PROTECTED = /^(approval|login|evidence|emergency[-_ ]?stop|protected)([:/_-]|$)/i

/** The field each action needs (CONTRACT 3.8). Returns the missing field, or undefined. */
function missingField(p: Input): string | undefined {
  switch (p.action) {
    case "open":
      return p.view_id ? undefined : "view_id"
    case "close":
    case "focus":
      return p.instance_id ? undefined : "instance_id"
    case "arrange":
      return p.arrangement ? undefined : "arrangement"
    case "start_run":
      return p.task ? undefined : "task"
    case "stop_run":
      return p.run_id ? undefined : "run_id"
    case "install_app":
      return p.app_source ? undefined : "app_source"
    case "fork_app":
      return p.fork_of ? undefined : "fork_of"
    default:
      return undefined
  }
}

function fail(error: string, message: string, metadata: Record<string, unknown> = {}) {
  return {
    title: `Shell: ${error}`,
    output: JSON.stringify({ ok: false, error, message }),
    metadata: { ok: false, error, ...metadata },
  }
}

export const ShellTool = Tool.define("shell", {
  description: DESCRIPTION,
  parameters: Params,
  async execute(params: Input, ctx) {
    if (params.action === "get_state") {
      const result = await ShellBridge.request({ sessionID: ctx.sessionID, kind: "get_state" }, { abort: ctx.abort })
      if ("ok" in result && !result.ok) return fail(result.error, "The screen state could not be read.")
      return { title: "Shell: state", output: JSON.stringify(result), metadata: { ok: true, action: "get_state" } }
    }

    const missing = missingField(params)
    if (missing) return fail("bad_request", `${params.action} needs ${missing}.`, { action: params.action })

    const viewIsProtected =
      (params.action === "open" && !!params.view_id && PROTECTED.test(params.view_id)) ||
      (params.action === "open" && params.params?.kind === "protected")
    if (viewIsProtected) {
      return fail("forbidden", "Approval, login, evidence and emergency-stop screens are raised by the system, not by you.", {
        action: params.action,
      })
    }

    // app_install class: a person approves every call; always: [] and ALWAYS_ASK mean no earlier answer, mode or task grant covers it.
    if (params.action === "install_app" || params.action === "fork_app") {
      const target = params.action === "install_app" ? params.app_source! : params.fork_of!
      await ctx.ask({
        permission: APP_INSTALL_PERMISSION,
        patterns: [`${params.action}:${target.app_id}`],
        always: [],
        metadata: {
          summary:
            params.action === "install_app"
              ? `Install app ${target.app_id} from ${params.app_source!.source}`
              : `Make your own copy of app ${target.app_id}`,
          shell: { action: params.action, target },
        },
      })
    }

    const { action: _action, ...fields } = params
    const command: ShellCommandV1 = {
      schema_id: SHELL_SCHEMA_ID,
      schema_version: SHELL_SCHEMA_VERSION,
      command_id: Identifier.ascending("tool"),
      action: params.action,
      ...fields,
    }
    const result = await ShellBridge.request({ sessionID: ctx.sessionID, kind: "command", command }, { abort: ctx.abort })
    if ("ok" in result && result.ok) {
      return {
        title: `Shell: ${params.action}`,
        output: JSON.stringify(result),
        metadata: { ok: true, action: params.action, command_id: result.command_id },
      }
    }
    const error = "ok" in result ? result.error : "bad_request"
    return fail(error, error === "no_app" ? "No app answered; the user has no app open for this session." : `The shell refused: ${error}.`, {
      action: params.action,
      command_id: command.command_id,
      receipt_id: "receipt_id" in result ? result.receipt_id : undefined,
    })
  },
})
