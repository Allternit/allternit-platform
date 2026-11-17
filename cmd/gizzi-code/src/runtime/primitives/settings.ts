/**
 * Settings (Primitives Wave D, D2): read and change the user's settings from
 * a chat. Shared core for the `settings` tool (runtime builtin
 * tools/builtins/settings.ts and the Ink adapter in
 * cli/ui/ink-app/tools/SettingsTool, which replaces the Ink ConfigTool).
 *
 * - Keys and their risk class come from one file:
 *   contracts/settings/definitions.json. They are read on demand
 *   (decision 14), never injected into every turn.
 *     safe:    applied by allternit-api (account keys) or by the client that
 *              sent the turn (device and runtime keys).
 *     confirm: security, privacy, instructions; applied only by the user's
 *              own tap, under their sign-in. Refused for automation runs.
 *     locked:  money, account; shown only as a link to Settings.
 * - The risk gate is enforced on the server (allternit-api
 *   `agent_settings_tools.rs`). This core never applies anything itself: it
 *   relays the server's outcomes. It also gates first, from the same file, so
 *   a locked key's value (a password, a key) never leaves this process, and a
 *   scheduled run's confirm request is refused without a round trip.
 * - Per-device keys (theme, chat font) come back `apply_on_device`: the
 *   client that sent the turn applies them once per tool-call id; other
 *   devices show a one-tap Apply (decision 13).
 * - write is refused in plan mode (loop/agent.ts, plan agent).
 *
 * Free of UI imports so the Ink adapter and bun tests can use it.
 */
import z from "zod/v4"
import contract from "../../../../../contracts/settings/definitions.json"
import { AppRoutingError } from "@/runtime/bots/app-bot-routing"
import { readSettings, writeSettings } from "@/runtime/bots/routing/settings"

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

  export type Scope = "account" | "device" | "runtime"

  export interface Definition {
    key: string
    class: RiskClass
    label: string
    description?: string
    /** "device" and "runtime" keys are applied by the client that sent the turn. */
    scope: Scope
    type: "boolean" | "string" | "enum" | "list"
    values?: string[]
    /** Longest string, or most list items. */
    max?: number
    /** Settings screen to open, "settings/<section>". */
    settings_path?: string
  }

  export interface ReadEntry extends Definition {
    /** Current value; absent for device and runtime keys and locked keys. */
    value?: unknown
  }

  export interface ChangeResult {
    key: string
    outcome: Outcome
    label?: string
    scope?: Scope
    /** Value before the change, when known (account keys), for Undo. */
    previous?: unknown
    value?: unknown
    /** Why it was invalid, or what the user needs to do. */
    reason?: string
    /** "automation_run": a scheduled run asked for a confirm key. */
    code?: string
    settings_path?: string
  }

  export interface Result {
    action: Input["action"]
    /** read */
    settings?: ReadEntry[]
    /** read: keys that don't exist */
    unknown?: string[]
    /** write */
    changes?: ChangeResult[]
    /** write: the tool call the device applier keys on */
    toolCallId?: string
  }

  export class NotReadyError extends Error {
    constructor() {
      super("settings is not available in this build yet.")
      this.name = "SettingsNotReadyError"
    }
  }

  /** Every key the tool knows, from contracts/settings/definitions.json. */
  export const DEFINITIONS: readonly Definition[] = (contract as unknown as { settings: Array<Definition & { store?: unknown }> }).settings.map(
    ({ store: _store, ...d }) => d,
  )

  export function definition(key: string): Definition | undefined {
    return DEFINITIONS.find((d) => d.key === key)
  }

  export const AUTOMATION_RUN_REFUSAL =
    "This run was started by a scheduled task, so it can't ask to change a security, privacy or instructions setting. Tell the user in your reply instead."

  /**
   * The local half of the gate: settles a change without the server when the
   * answer can't depend on it (unknown key, locked key, a confirm key from an
   * automation run). Undefined means "ask the server".
   */
  export function precheck(change: Change, opts: { automationRun?: boolean } = {}): ChangeResult | undefined {
    const def = definition(change.key)
    if (!def) {
      return { key: change.key, outcome: "invalid", reason: `There's no setting "${change.key}". Read the settings to see the keys.` }
    }
    const base = { key: def.key, label: def.label, scope: def.scope }
    if (def.class === "locked") {
      return {
        ...base,
        outcome: "locked",
        reason: "This is changed only in Settings. Give the user the link; you can't change it.",
        settings_path: def.settings_path,
      }
    }
    if (def.class === "confirm" && opts.automationRun) {
      return { ...base, outcome: "invalid", code: "automation_run", reason: AUTOMATION_RUN_REFUSAL }
    }
    return undefined
  }

  /** The value check the server makes, for the no-account fallback. Undefined when the value fits. */
  export function checkValue(def: Definition, value: unknown): string | undefined {
    switch (def.type) {
      case "boolean":
        return typeof value === "boolean" ? undefined : `${def.key} takes true or false.`
      case "enum":
        return typeof value === "string" && (def.values ?? []).includes(value)
          ? undefined
          : `${def.key} takes one of: ${(def.values ?? []).join(", ")}.`
      case "string":
        if (typeof value !== "string") return `${def.key} takes text.`
        return [...value].length <= (def.max ?? 1000) ? undefined : `${def.key} can be at most ${def.max ?? 1000} characters.`
      case "list":
        if (!Array.isArray(value) || !value.every((v) => typeof v === "string")) return `${def.key} takes a list of text items.`
        return value.length <= (def.max ?? 200) ? undefined : `${def.key} can hold at most ${def.max ?? 200} items.`
    }
  }

  function noAccount(error: unknown): boolean {
    return error instanceof AppRoutingError && error.code === "no_credential"
  }

  function message(error: unknown): string {
    return error instanceof Error ? error.message : String(error)
  }

  export async function read(sessionID: string, keys: string[] | undefined, signal?: AbortSignal): Promise<Result> {
    signal?.throwIfAborted()
    try {
      const r = await readSettings(sessionID, keys ? { keys } : {})
      return { action: "read", settings: r.settings ?? [], unknown: r.unknown?.length ? r.unknown : undefined }
    } catch (error) {
      // Not signed in (Gizzi Code without an account): the definitions still
      // tell the model what exists; account values can't be read.
      if (!noAccount(error)) throw error
      const wanted = keys ? DEFINITIONS.filter((d) => keys.includes(d.key)) : [...DEFINITIONS]
      const unknown = keys?.filter((k) => !definition(k))
      return { action: "read", settings: wanted, unknown: unknown?.length ? unknown : undefined }
    }
  }

  /**
   * `toolCallID`: the sending client applies per-device keys once per call id.
   * `automationRun`: a scheduled task started this turn (from the user
   * message's metadata, never from the model).
   */
  export async function write(
    sessionID: string,
    toolCallID: string,
    changes: Change[],
    signal?: AbortSignal,
    opts: { automationRun?: boolean } = {},
  ): Promise<Result> {
    signal?.throwIfAborted()
    if (changes.length === 0 || changes.length > MAX_CHANGES) {
      throw new Error(`Send 1 to ${MAX_CHANGES} changes.`)
    }
    const results: (ChangeResult | undefined)[] = changes.map((c) => precheck(c, opts))
    const pending = changes.map((change, index) => ({ change, index })).filter(({ index }) => !results[index])
    if (pending.length) {
      try {
        const r = await writeSettings(sessionID, {
          toolCallId: toolCallID,
          changes: pending.map((p) => p.change),
          ...(opts.automationRun ? { automationRun: true } : {}),
        })
        const fromServer = r.changes ?? []
        pending.forEach(({ change, index }, i) => {
          const got = fromServer[i]
          results[index] =
            got && got.key === change.key ? got : { key: change.key, outcome: "invalid", reason: "No result from Allternit for this change." }
        })
      } catch (error) {
        signal?.throwIfAborted()
        // Not signed in: device and runtime keys are still the sending
        // client's to apply; account keys need the account.
        for (const { change, index } of pending) {
          const def = definition(change.key)!
          const base = { key: def.key, label: def.label, scope: def.scope }
          const bad = checkValue(def, change.value)
          results[index] = bad
            ? { ...base, outcome: "invalid", reason: bad }
            : noAccount(error) && def.class === "safe" && def.scope !== "account"
              ? { ...base, outcome: "apply_on_device", value: change.value }
              : { ...base, outcome: "invalid", reason: `Not changed: ${message(error)}` }
        }
      }
    }
    return { action: "write", toolCallId: toolCallID, changes: results as ChangeResult[] }
  }
}
