/**
 * Settings (Primitives Wave D, D2): read and change the user's settings from
 * a chat. Shared core for the `settings` tool (runtime builtin
 * tools/builtins/settings.ts and the Ink adapter in
 * cli/ui/ink-app/tools/SettingsTool, which replaces the Ink ConfigTool).
 *
 * - Keys and their risk class come from one file:
 *   contracts/settings/definitions.json. They are read on demand
 *   (decision 14), never injected into every turn.
 *     safe:    the runtime applies the change.
 *     confirm: security, privacy, instructions; applied only by the user's
 *              own tap, under their sign-in.
 *     locked:  money, account; shown only as a link to Settings.
 * - The risk gate is enforced on the server (allternit-api
 *   `agent_settings_tools.rs`), not only here: a confirm or locked key is
 *   never applied by the runtime, under allow-all or an automation origin.
 * - Per-device keys (theme, chat font) come back `apply_on_device`: the
 *   client that sent the turn applies them once per tool-call id; other
 *   devices show a one-tap Apply (decision 13).
 * - write is refused in plan mode (loop/agent.ts, plan agent).
 *
 * W2 stub: final signatures and limits; lane S fills the bodies.
 * Free of UI imports so the Ink adapter and bun tests can use it.
 */
import z from "zod/v4"

export namespace Settings {
  export const MAX_CHANGES = 5

  export const Change = z.object({
    key: z.string().min(1).max(100).describe('A setting key from a settings read, e.g. "appearance.theme"'),
    value: z.unknown().describe("The new value, of the type the read reported"),
  })
  export type Change = z.infer<typeof Change>

  export const Input = z.object({
    action: z.enum(["read", "write"]).describe("read settings and their allowed values, or write changes"),
    keys: z.array(z.string().min(1).max(100)).max(50).optional().describe("Keys to read (read; omit to list every key)"),
    changes: z.array(Change).min(1).max(MAX_CHANGES).optional().describe(`Changes to make (write, max ${MAX_CHANGES})`),
  })
  export type Input = z.infer<typeof Input>

  export type RiskClass = "safe" | "confirm" | "locked"

  /** What happened to one change. */
  export type Outcome = "applied" | "apply_on_device" | "needs_confirmation" | "locked" | "invalid"

  export interface Definition {
    key: string
    class: RiskClass
    label: string
    /** "device" keys are applied by the client that sent the turn. */
    scope: "account" | "device" | "runtime"
    type: "boolean" | "string" | "enum" | "list"
    values?: string[]
    /** Settings screen to open for confirm and locked keys. */
    settings_path?: string
  }

  export interface ReadEntry extends Definition {
    /** Current value; absent for per-device keys the runtime can't see. */
    value?: unknown
  }

  export interface ChangeResult {
    key: string
    outcome: Outcome
    /** Value before the change, for Undo (applied). */
    previous?: unknown
    value?: unknown
    /** Why it was invalid, or what the user needs to do. */
    reason?: string
    settings_path?: string
  }

  export interface Result {
    action: Input["action"]
    /** read */
    settings?: ReadEntry[]
    /** write */
    changes?: ChangeResult[]
  }

  export class NotReadyError extends Error {
    constructor() {
      super("settings is not available in this build yet.")
      this.name = "SettingsNotReadyError"
    }
  }

  export async function read(_sessionID: string, _keys: string[] | undefined, _signal?: AbortSignal): Promise<Result> {
    throw new NotReadyError()
  }

  /** `toolCallID`: the sending client applies per-device keys once per call id. */
  export async function write(_sessionID: string, _toolCallID: string, _changes: Change[], _signal?: AbortSignal): Promise<Result> {
    throw new NotReadyError()
  }
}
