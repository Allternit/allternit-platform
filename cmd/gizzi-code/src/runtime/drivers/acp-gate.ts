/**
 * Spawn gate for ACP-driven CLIs (kimi, gemini). Those CLIs have no hook
 * mechanism, but every tool call they make outside auto-approve arrives here
 * as `session/request_permission`. We answer each one immediately from the
 * same decision path the hooked harnesses use (`allternit-commrails hook
 * claude-pretool`: hard floor + Gate 2 when a WIH is bound), so nothing waits
 * on a person and a headless turn never hangs.
 */
import { existsSync } from "node:fs"
import { spawn } from "node:child_process"
import { delimiter, join } from "node:path"

export type AcpToolCall = {
  kind?: unknown
  title?: unknown
  rawInput?: unknown
  locations?: unknown
}

export type AcpGateVerdict = { allow: true } | { allow: false; reason: string }

/** Map an ACP tool call onto the hook payload `{tool_name, tool_input, cwd}`. */
export function acpToolToHookPayload(toolCall: AcpToolCall, cwd: string, sessionId?: string) {
  const kind = String(toolCall.kind ?? "")
  const raw = (toolCall.rawInput && typeof toolCall.rawInput === "object" ? toolCall.rawInput : {}) as Record<
    string,
    unknown
  >
  const locations = Array.isArray(toolCall.locations) ? (toolCall.locations as Array<{ path?: unknown }>) : []
  const locPath = typeof locations[0]?.path === "string" ? (locations[0].path as string) : undefined
  const command =
    typeof raw.command === "string"
      ? raw.command
      : Array.isArray(raw.command)
        ? (raw.command as unknown[]).map(String).join(" ")
        : typeof raw.cmd === "string"
          ? raw.cmd
          : undefined
  const filePath = (typeof raw.file_path === "string" ? raw.file_path : undefined) ?? (typeof raw.path === "string" ? raw.path : undefined) ?? locPath

  if (kind === "execute" || (command && !["edit", "delete", "move"].includes(kind))) {
    // No recoverable command: hand the floor the title so it can still match.
    return {
      tool_name: "Bash",
      tool_input: { command: command ?? String(toolCall.title ?? "") },
      cwd,
      session_id: sessionId,
    }
  }
  if (["edit", "delete", "move"].includes(kind)) {
    return { tool_name: "Write", tool_input: { file_path: filePath ?? "" }, cwd, session_id: sessionId }
  }
  return {
    tool_name: kind === "read" || kind === "search" ? "Read" : kind === "fetch" ? "WebFetch" : "Task",
    tool_input: filePath ? { file_path: filePath } : {},
    cwd,
    session_id: sessionId,
  }
}

/** `$ALLTERNIT_COMMRAILS_BIN`, then `allternit-commrails` on PATH. */
export function findCommrailsBin(env: NodeJS.ProcessEnv = process.env): string | undefined {
  const explicit = env.ALLTERNIT_COMMRAILS_BIN
  if (explicit && existsSync(explicit)) return explicit
  for (const dir of (env.PATH ?? "").split(delimiter)) {
    if (!dir) continue
    const cand = join(dir, "allternit-commrails")
    if (existsSync(cand)) return cand
  }
  return undefined
}

/** Interpret the hook's stdout: silence = allow, a `deny` decision = deny. */
export function parseHookOutput(stdout: string, exitCode: number | null): AcpGateVerdict {
  if (exitCode === 2) return { allow: false, reason: "gate exited 2 (fail closed)" }
  const text = stdout.trim()
  if (!text) return exitCode === 0 ? { allow: true } : { allow: false, reason: `gate exited ${exitCode} (fail closed)` }
  try {
    const out = JSON.parse(text) as { hookSpecificOutput?: { permissionDecision?: string; permissionDecisionReason?: string } }
    if (out.hookSpecificOutput?.permissionDecision === "deny") {
      return { allow: false, reason: out.hookSpecificOutput.permissionDecisionReason ?? "denied by Allternit spawn gate" }
    }
    return { allow: true }
  } catch {
    return { allow: false, reason: "unreadable gate output (fail closed)" }
  }
}

/**
 * Ask the gate about one ACP tool call. Returns `undefined` when no gate
 * binary is installed, so the caller keeps its existing behaviour instead of
 * denying everything.
 */
export async function acpGateDecision(opts: {
  toolCall: AcpToolCall
  cwd: string
  harness: string
  sessionId?: string
  wihId?: string
  root?: string
  bin?: string
  timeoutMs?: number
}): Promise<AcpGateVerdict | undefined> {
  const bin = opts.bin ?? findCommrailsBin()
  if (!bin) return undefined
  const args = ["--root", opts.root ?? opts.cwd, "hook", "claude-pretool", "--harness", opts.harness, "--workspace", opts.cwd]
  if (opts.wihId) args.push("--wih", opts.wihId)
  const payload = JSON.stringify(acpToolToHookPayload(opts.toolCall, opts.cwd, opts.sessionId))
  return await new Promise<AcpGateVerdict>((resolve) => {
    const child = spawn(bin, args, { stdio: ["pipe", "pipe", "ignore"] })
    let stdout = ""
    const timer = setTimeout(() => {
      child.kill()
      resolve({ allow: false, reason: "gate timed out (fail closed)" })
    }, opts.timeoutMs ?? 30_000)
    child.stdout.on("data", (d) => (stdout += d.toString()))
    child.on("error", () => {
      clearTimeout(timer)
      resolve({ allow: false, reason: "gate failed to start (fail closed)" })
    })
    child.on("close", (code) => {
      clearTimeout(timer)
      resolve(parseHookOutput(stdout, code))
    })
    child.stdin.end(payload)
  })
}
