// Portions adapted from Reason Network (reason-machines/reason-network), Apache-2.0. Copyright 2026 Reason Machines, Inc.
/**
 * `gizzi hook <harness> <event>`: turn-tier delivery (agent network C2 step 6).
 *
 * A harness runs this from its own hooks (installed by the Factory engine's
 * `delivery.rs`). It reads pending mail for the session, prints it in the JSON
 * shape that harness expects (Reason's `hook.rs` table), and marks the
 * messages read only after stdout was written. It never fails the harness:
 * any error prints nothing and exits 0.
 */
import type { CommandModule } from "yargs"
import { basename } from "node:path"
import { markRead, readPending, sessionKey, type InboxMessage } from "@/runtime/live/inbox"

type Ev = "start" | "prompt" | "tool" | "stop" | "end" | "doing"

export function normalizeEvent(event: string): Ev | undefined {
  switch (event) {
    case "session-start":
      return "start"
    case "user-prompt-submit":
      return "prompt"
    case "post-tool-use":
      return "tool"
    case "stop":
    case "poll":
      return "stop"
    case "session-end":
      return "end"
    case "doing":
      return "doing"
    default:
      return undefined
  }
}

/** Prompt-submit hooks in Cursor and Copilot can't add context; opencode's status call shows nothing. */
export function takesMail(harness: string, event: string): boolean {
  if ((harness === "cursor" || harness === "copilot" || harness === "grok") && event === "user-prompt-submit") return false
  if (harness === "grok" && event === "session-start") return false
  return event !== "status" && event !== "doing"
}

/** The JSON (or text) each harness expects from this hook to put `text` in front of the model. */
export function formatFor(harness: string, ev: Ev, text: string): string {
  const specific = (name: string) => ({ hookSpecificOutput: { hookEventName: name, additionalContext: text } })
  const block = { decision: "block", reason: text }
  const key = `${harness}:${ev}`
  switch (key) {
    case "codex:start":
    case "gemini:start":
      return JSON.stringify(specific("SessionStart"))
    case "codex:prompt":
      return JSON.stringify(specific("UserPromptSubmit"))
    case "codex:tool":
    case "grok:tool":
      return JSON.stringify(specific("PostToolUse"))
    case "gemini:prompt":
      return JSON.stringify(specific("BeforeAgent"))
    case "gemini:tool":
      return JSON.stringify(specific("AfterTool"))
  }
  if (ev === "stop" && ["codex", "gemini", "copilot", "grok"].includes(harness)) return JSON.stringify(block)
  if (harness === "cursor") return JSON.stringify(ev === "stop" ? { followup_message: text } : { additional_context: text })
  if (harness === "copilot") return JSON.stringify({ additionalContext: text })
  return text
}

/** The text a hook prints for pending mail. */
export function mailText(pending: InboxMessage[]): string {
  return pending.map((m) => m.envelope).join("\n\n")
}

async function readStdin(timeoutMs = 1000): Promise<Record<string, unknown>> {
  if (process.stdin.isTTY) return {}
  const chunks: Buffer[] = []
  const done = new Promise<void>((resolve) => {
    process.stdin.on("data", (c) => chunks.push(Buffer.from(c)))
    process.stdin.on("end", () => resolve())
    process.stdin.on("error", () => resolve())
  })
  await Promise.race([done, new Promise<void>((r) => setTimeout(r, timeoutMs))])
  try {
    return JSON.parse(Buffer.concat(chunks).toString("utf8")) as Record<string, unknown>
  } catch {
    return {}
  }
}

/** One hook run. Returns what to print (or undefined) and the ids to mark read once it was written. */
export async function runHook(
  harness: string,
  event: string,
  input: Record<string, unknown> = {},
): Promise<{ out?: string; ids: string[]; key?: string }> {
  const ev = normalizeEvent(event)
  if (!ev || !takesMail(harness, event)) return { ids: [] }
  if (harness === "grok" && (input.subagentType || (ev === "stop" && input.reason !== "end_turn"))) return { ids: [] }
  const roots = input.workspace_roots as string[] | undefined
  const cwd =
    (input.cwd as string | undefined) ??
    roots?.[0] ??
    process.env.CURSOR_PROJECT_DIR ??
    process.env.GEMINI_CWD ??
    process.env.CLAUDE_PROJECT_DIR ??
    process.cwd()
  const key = sessionKey(harness, cwd)
  const pending = await readPending(key)
  if (!pending.length) {
    if (ev === "start") {
      const text = `Allternit: this session is "${key}" (${basename(cwd)}). Teammates' agents can message you; list_team and send_message reach them.`
      return { out: formatFor(harness, ev, text), ids: [] }
    }
    return { ids: [] }
  }
  const text = mailText(pending)
  return { out: formatFor(harness, ev, text), ids: pending.map((m) => m.id), key }
}

export const HookCommand: CommandModule<{}, { harness: string; event: string }> = {
  command: "hook <harness> <event>",
  describe: false,
  builder: (yargs) =>
    yargs
      .positional("harness", { type: "string", demandOption: true, describe: "codex | gemini | cursor | copilot | opencode | pi | grok" })
      .positional("event", { type: "string", demandOption: true, describe: "session-start | user-prompt-submit | post-tool-use | stop | session-end | poll" }),
  handler: async (argv) => {
    try {
      const harness = String(argv.harness).toLowerCase()
      const { out, ids, key } = await runHook(harness, String(argv.event), await readStdin())
      if (out) {
        // Mark read only once the text reached stdout.
        await new Promise<void>((resolve) => process.stdout.write(out + "\n", () => resolve()))
        if (key) await markRead(key, ids).catch(() => {})
      }
    } catch {
      // A hook must never fail the harness.
    }
    process.exit(0)
  },
}
