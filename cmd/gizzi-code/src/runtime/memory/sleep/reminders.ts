/**
 * Allternit Sleep reminders (prospective memory): a reminder the person asked
 * for fires on the server; Gizzi hands it to the model once.
 *
 * At the start of a top-level user turn the runtime reads
 * GET /memory/sleep/context (the server marks fired reminders done when it
 * returns them) and keeps the text for THIS turn. The text rides as a
 * message-layer tail block (PromptSegments.withTail on the runtime, an
 * appended meta user message in the TUI loop), never in the system prompt, so
 * the cached prefix is untouched.
 *
 * Why await at turn start (and not prefetch on idle): the server marks a
 * reminder delivered the moment it is read, so a prefetch that is not
 * certainly delivered would lose it. The wait is capped at
 * REMINDER_TIMEOUT_MS; the turn never waits longer. If the answer arrives
 * after the cap it is still kept for the same turn (the tail is rebuilt each
 * step), so nothing the server already marked done is dropped.
 *
 * Off, signed out, 404, error, empty: nothing is added and nothing throws.
 */
export const REMINDER_TIMEOUT_MS = 1_500

export interface Intention {
  id?: string
  content?: string
  cue_kind?: string
  cue?: string | null
  due_at?: string | null
  fired_at?: string | null
}

export interface ReminderDeps {
  /** Signed in and Sleep on (cached status). */
  enabled?: () => Promise<boolean>
  /** The context call; resolves the intentions, or throws / returns [] on any failure. */
  fetchIntentions?: (sessionID: string, model: string | undefined, timeoutMs: number) => Promise<Intention[]>
  timeoutMs?: number
}

const clean = (s: string) => s.replace(/<\/?system-reminder>/gi, "").replace(/\s+/g, " ").trim().slice(0, 500)

/** One reminder as a tail line. */
export function reminderLine(i: Intention): string | undefined {
  const content = typeof i.content === "string" ? clean(i.content) : ""
  if (!content) return undefined
  const when = i.due_at || i.fired_at
  const notes: string[] = []
  if (when) notes.push(`due ${clean(String(when)).slice(0, 10)}`)
  if (i.cue) notes.push(`cue: "${clean(String(i.cue))}"`)
  const suffix = notes.length ? ` (${notes.join(", ")})` : ""
  return `The user asked to be reminded: "${content}"${suffix}. Mention it naturally if relevant now.`
}

export function reminderTail(items: Intention[]): string[] {
  return items.map(reminderLine).filter((x): x is string => !!x)
}

const realEnabled = async () => (await import("./client")).sleepEnabled()
const realFetch = async (sessionID: string, model: string | undefined, timeoutMs: number): Promise<Intention[]> => {
  const { sleepCall } = await import("./client")
  const q = `session_id=${encodeURIComponent(sessionID)}${model ? `&model=${encodeURIComponent(model)}` : ""}`
  const out = await sleepCall<{ intentions?: Intention[] }>("GET", `sleep/context?${q}`, undefined, timeoutMs)
  return out.kind === "ok" && Array.isArray(out.body?.intentions) ? out.body.intentions : []
}

export function createReminders(deps: ReminderDeps = {}) {
  const enabled = deps.enabled ?? realEnabled
  const fetchIntentions = deps.fetchIntentions ?? realFetch
  const timeoutMs = deps.timeoutMs ?? REMINDER_TIMEOUT_MS
  const pending = new Map<string, string[]>()
  const generation = new Map<string, number>()

  return {
    /**
     * A top-level user turn starts: replace last turn's reminders with what the
     * server has now. Resolves within `timeoutMs`; never throws.
     */
    async turnStart(sessionID: string, model?: string): Promise<void> {
      const gen = (generation.get(sessionID) ?? 0) + 1
      generation.set(sessionID, gen)
      pending.delete(sessionID)
      let handle: ReturnType<typeof setTimeout> | undefined
      try {
        if (!(await Promise.race([enabled(), new Promise<boolean>((r) => (handle = setTimeout(() => r(false), timeoutMs)))]))) return
        clearTimeout(handle)
        const work = fetchIntentions(sessionID, model, timeoutMs)
          .then((items) => {
            // Late answers still belong to this turn, never to a newer one.
            if (generation.get(sessionID) !== gen) return
            const tail = reminderTail(Array.isArray(items) ? items : [])
            if (tail.length) pending.set(sessionID, tail)
          })
          .catch(() => {})
        let cap: ReturnType<typeof setTimeout> | undefined
        await Promise.race([work, new Promise<void>((r) => (cap = setTimeout(r, timeoutMs)))])
        clearTimeout(cap)
      } catch {
        // nothing added
      } finally {
        clearTimeout(handle)
      }
    },

    /** Tail lines for the current turn (empty when none). Safe to call every step. */
    tail(sessionID: string): string[] {
      return pending.get(sessionID) ?? []
    },

    /** The turn ended. */
    clear(sessionID: string): void {
      pending.delete(sessionID)
    },
  }
}

export type Reminders = ReturnType<typeof createReminders>

/** The process-wide reminders (real status check, real API). */
export const SleepReminders: Reminders = createReminders()
