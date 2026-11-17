/**
 * Heartbeat verdicts: how a heartbeat run tells the scheduler whether to
 * bother the user.
 *
 * A heartbeat is a recurring check that runs in its own fresh session. Its
 * reply must end with one verdict line:
 *   NOTIFY: <line>   post <line> to the automation's thread and push it
 *   DONT_NOTIFY      nothing worth saying; a quiet `skipped` run
 *   DONE: <line>     post and push <line>, then mark the automation complete
 * No verdict counts as a failed run (the failure streak pauses the automation).
 *
 * Two parsers read this: this file (the runtime /cron fallback) and
 * allternit-api's routine scheduler. Both are tested against the same vectors
 * in contracts/automation/heartbeat-verdict.json, and both append the same
 * instructions (contracts/automation/heartbeat-instructions.txt, which must
 * equal heartbeatInstructions() here; a test checks it).
 *
 * Free of runtime state so the Ink adapter and bun tests can use it.
 */
export namespace Heartbeat {
  export type Verdict =
    | { kind: "notify"; line: string }
    | { kind: "dont_notify" }
    | { kind: "done"; line: string }
    | { kind: "none" }

  /** A line that is only a code fence or decoration, skipped when finding the last line. */
  const FILLER = /^(`{3,}|~{3,}|[-*_=]{3,}|[*_`>#\s]*)$/
  /** Markdown wrapped around the verdict: bold, italics, inline code, quotes, bullets, headings. */
  const LEAD = /^[\s>*_`#-]+/
  const TRAIL = /[\s*_`]+$/
  const VERDICT = /^(dont[_ ]notify|don't[_ ]notify|do[_ ]not[_ ]notify|notify|done)(?![a-z_'])\s*(.*)$/i

  /**
   * The verdict on the last non-empty line of `reply` (trailing code fences
   * and markdown rules ignored). The keyword is case-insensitive. NOTIFY needs
   * a message after ":" or "-"; DONE's message may be empty; text after
   * DONT_NOTIFY is ignored. Anything else is `none`.
   */
  export function parseVerdict(reply: string | null | undefined): Verdict {
    if (!reply) return { kind: "none" }
    const lines = reply.split(/\r?\n/)
    let last: string | undefined
    for (let i = lines.length - 1; i >= 0; i--) {
      const line = lines[i]!.trim()
      if (!line || FILLER.test(line)) continue
      last = line
      break
    }
    if (!last) return { kind: "none" }
    const text = last.replace(LEAD, "").replace(TRAIL, "")
    const m = VERDICT.exec(text)
    if (!m) return { kind: "none" }
    const keyword = m[1]!.toLowerCase().replace(/[ ']/g, "_")
    const rest = m[2]!.trim()
    if (keyword !== "notify" && keyword !== "done") return { kind: "dont_notify" }
    // NOTIFY / DONE take their message after a colon or a dash.
    let line: string
    if (!rest || /^[.!;,]+$/.test(rest)) line = ""
    else {
      const sep = /^[:\-–—]\s*/.exec(rest)
      if (!sep) return { kind: "none" }
      line = rest.slice(sep[0].length).replace(/^[\s*_`]+/, "").replace(TRAIL, "").trim()
    }
    if (keyword === "done") return { kind: "done", line }
    if (!line) return { kind: "none" }
    return { kind: "notify", line }
  }

  const INSTRUCTIONS = [
    "This is a scheduled check (a heartbeat). It runs on its own, in a fresh session, and the user is probably not watching.",
    "",
    "Do the check described above. Then end your reply with exactly one of these as the very last line:",
    "- NOTIFY: <one short line for the user>  when something they would want to hear about happened: the thing finished, failed, changed in a way that matters, or needs them to act.",
    "- DONT_NOTIFY  when nothing worth their attention changed: still pending, still running, still healthy, same as last time.",
    "- DONE: <one short line for the user>  when the reason for this check is over (the thing finished or no longer matters). The user is told, and this scheduled task stops.",
    "",
    "Stay quiet unless it matters. Do not send routine status updates. Write the line so it makes sense on a phone lock screen, without the rest of your reply.",
    "Anything that needs the user's approval stops this run and asks them, so do not start work that needs approval.",
  ].join("\n")

  /** Appended to a heartbeat run's prompt: the verdict contract, in our words. */
  export function heartbeatInstructions(): string {
    return INSTRUCTIONS
  }

  /** The prompt a heartbeat run receives: the automation's prompt, then the instructions. */
  export function runPrompt(prompt: string): string {
    return `${prompt.trim()}\n\n---\n${INSTRUCTIONS}`
  }
}

export const parseVerdict = Heartbeat.parseVerdict
export const heartbeatInstructions = Heartbeat.heartbeatInstructions
