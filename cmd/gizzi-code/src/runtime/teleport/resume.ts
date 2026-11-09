/**
 * Resume a teleported session natively on this computer: the CLI's own `--resume`
 * (flags from cli-session-flags, the same ones the local CLI driver uses), the
 * `[Teleport handoff]` text first, then the queued prompts in order. The caller
 * hears about the first output as soon as it appears; the rest keeps running.
 */
import { VendorSession } from "@/runtime/session/vendor-session"
import { claudeSessionFlags } from "@/runtime/drivers/cli-session-flags"
import type { TeleportHarness } from "@/runtime/teleport/teleport"

export interface ResumeArgs {
  /** The gizzi session the vendor id is recorded under (falls back to the native id). */
  sessionID: string
  harness: TeleportHarness
  nativeId: string
  cwd: string
  model: string
  effort?: string
  handoff: string
  queued: string[]
}

export interface ResumeStarted {
  vendorSessionId: string
  /** Resolves when every prompt has run; rejects if one fails. */
  done: Promise<void>
}

/** argv for one turn on a resumed native session. */
export function resumeArgv(args: Pick<ResumeArgs, "harness" | "model" | "effort"> & { vendorSessionId: string }, prompt: string): string[] {
  if (args.harness === "claude") {
    return [
      "claude",
      ...claudeSessionFlags({ vendorSessionId: args.vendorSessionId }),
      "--model",
      args.model,
      ...(args.effort ? ["--effort", args.effort] : []),
      "--output-format",
      "stream-json",
      "--verbose",
      "-p",
      prompt,
    ]
  }
  return [
    "codex",
    "exec",
    "resume",
    args.vendorSessionId,
    "-m",
    args.model,
    ...(args.effort ? ["-c", `model_reasoning_effort="${args.effort}"`] : []),
    "--json",
    prompt,
  ]
}

/** One turn; calls `onOutput` for the first stdout bytes and returns the vendor session id seen. */
async function turn(argv: string[], cwd: string, onOutput: () => void): Promise<string | undefined> {
  const proc = Bun.spawn(argv, { cwd, stdout: "pipe", stderr: "pipe", stdin: "ignore" })
  let seen: string | undefined
  let buffer = ""
  let first = true
  const decoder = new TextDecoder()
  for await (const chunk of proc.stdout as unknown as AsyncIterable<Uint8Array>) {
    if (first) {
      first = false
      onOutput()
    }
    buffer += decoder.decode(chunk, { stream: true })
    const lines = buffer.split("\n")
    buffer = lines.pop() ?? ""
    for (const line of lines) {
      try {
        const evt = JSON.parse(line) as { session_id?: string; thread_id?: string }
        seen = evt.session_id ?? evt.thread_id ?? seen
      } catch {}
    }
  }
  const code = await proc.exited
  if (code !== 0) {
    const err = await new Response(proc.stderr as ReadableStream).text().catch(() => "")
    throw new Error(`${argv[0]} exited with ${code}${err ? `: ${err.trim().slice(0, 300)}` : ""}`)
  }
  return seen
}

/**
 * Start the resume. Resolves once the handoff turn has produced its first
 * output, with the vendor session id already stored durably; later prompts keep
 * running in `done`.
 */
export async function resumeNative(args: ResumeArgs): Promise<ResumeStarted> {
  const prompts = [`[Teleport handoff]\n${args.handoff}`, ...args.queued]
  let vendorSessionId = args.nativeId
  let markFirst: () => void = () => {}
  let failFirst: (e: unknown) => void = () => {}
  const first = new Promise<void>((resolve, reject) => {
    markFirst = resolve
    failFirst = reject
  })
  const done = (async () => {
    for (const [i, prompt] of prompts.entries()) {
      const seen = await turn(resumeArgv({ ...args, vendorSessionId }, prompt), args.cwd, i === 0 ? markFirst : () => {})
      if (seen && seen !== vendorSessionId) vendorSessionId = seen
      // Durable from the first turn on: a crash later still resumes the right conversation.
      await VendorSession.set(args.sessionID, args.harness, vendorSessionId)
    }
  })()
  done.catch(failFirst)
  await first
  await VendorSession.set(args.sessionID, args.harness, vendorSessionId)
  return { vendorSessionId, done }
}
