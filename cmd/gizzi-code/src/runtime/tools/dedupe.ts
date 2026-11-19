import { RuntimeTelemetry } from "@/runtime/telemetry"

interface SessionState {
  messageID: string
  pending: Map<string, Promise<unknown>>
  lastKey?: string
  repeatCount: number
  touchedAt: number
}

const sessions = new Map<string, SessionState>()
const MAX_SESSION_STATES = 256

function canonical(value: unknown): string {
  if (value === undefined) return "undefined"
  if (value === null || typeof value !== "object") return JSON.stringify(value) ?? String(value)
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`
  return `{${Object.entries(value as Record<string, unknown>)
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([key, entry]) => `${JSON.stringify(key)}:${canonical(entry)}`)
    .join(",")}}`
}

function appendReminder<T>(result: T, count: number): T {
  if (!result || typeof result !== "object" || !("output" in result) || typeof result.output !== "string") return result
  let reminder: string | undefined
  if (count >= 8) {
    reminder =
      "The same tool call has been repeated many times. Stop calling tools and summarize the blocker, attempted approaches, and the exact input needed to continue."
  } else if (count >= 5) {
    reminder =
      "This exact tool call is repeating. Choose one next action: run a cheap falsification check, ask for a missing input, or conclude from existing evidence."
  } else if (count >= 3) {
    reminder =
      "This exact tool call has repeated. Before another call, state what new information it should produce and choose a different action if the current result already answers it."
  }
  if (!reminder) return result
  return {
    ...result,
    output: `${result.output}\n\n<system-reminder>\n${reminder}\n</system-reminder>`,
  } as T
}

function state(sessionID: string, messageID: string): SessionState {
  const existing = sessions.get(sessionID)
  if (existing) {
    existing.touchedAt = Date.now()
    if (existing.messageID !== messageID) {
      existing.messageID = messageID
      existing.pending.clear()
    }
    return existing
  }
  if (sessions.size >= MAX_SESSION_STATES) {
    const oldest = [...sessions.entries()].sort((a, b) => a[1].touchedAt - b[1].touchedAt)[0]
    if (oldest) sessions.delete(oldest[0])
  }
  const created: SessionState = {
    messageID,
    pending: new Map(),
    repeatCount: 0,
    touchedAt: Date.now(),
  }
  sessions.set(sessionID, created)
  return created
}

export namespace ToolDedupe {
  export async function execute<T>(input: {
    sessionID: string
    messageID: string
    tool: string
    args: unknown
    run: () => Promise<T>
  }): Promise<T> {
    const current = state(input.sessionID, input.messageID)
    // A tool-call label (3-8 words for the person watching) never changes what runs, so it stays out of the key.
    const args =
      input.args && typeof input.args === "object" && !Array.isArray(input.args) && "label" in input.args
        ? Object.fromEntries(Object.entries(input.args as Record<string, unknown>).filter(([k]) => k !== "label"))
        : input.args
    const key = `${input.tool}:${canonical(args)}`
    const pending = current.pending.get(key)
    if (pending) {
      RuntimeTelemetry.track("tool_call_duplicate", { kind: "same_step" })
      return pending as Promise<T>
    }

    const run = input.run().then((result) => {
      if (current.lastKey === key) current.repeatCount += 1
      else {
        current.lastKey = key
        current.repeatCount = 1
      }
      if (current.repeatCount > 1) {
        RuntimeTelemetry.track("tool_call_duplicate", { kind: "cross_step" })
      }
      return appendReminder(result, current.repeatCount)
    })
    current.pending.set(key, run)
    // Keep completed calls in the current step so later duplicate calls share
    // the original promise and never execute a side effect twice.
    return run
  }

  /** @internal */
  export function reset() {
    sessions.clear()
  }
}
