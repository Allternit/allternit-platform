/**
 * Spawn gate for ACP-driven CLIs (kimi, gemini). Those CLIs have no hook
 * mechanism, but every tool call they make outside auto-approve arrives here
 * as `session/request_permission`. We answer each one immediately from the
 * same decision path the hooked harnesses use (`allternit-factory internal
 * core hook claude-pretool`: hard floor + Gate 2 when a WIH is bound), so nothing waits
 * on a person and a headless turn never hangs.
 * While the bound run replays (effects: recorded_only) that decision path
 * denies every tool with "replay: recorded result served by the gate"; the
 * gate's post-call step serves the recorded result instead.
 */
import { existsSync } from "node:fs"
import { spawn } from "node:child_process"
import { join } from "node:path"
import { locateEngine } from "@/cli/factory/engine"
import { Catastrophic } from "@/runtime/tools/guard/permission/catastrophic"

export type AcpToolCall = {
  kind?: unknown
  title?: unknown
  rawInput?: unknown
  locations?: unknown
}

export type AcpGateVerdict = { allow: true; fallback?: boolean } | { allow: false; reason: string; fallback?: boolean }

/** Plan mode is read-only: same rule as PermissionNext.evaluatePolicy. */
export function planModeDenies(mode: string | undefined, permission: string): boolean {
  return mode === "plan" && permission !== "read"
}

/** In-process floor used when the Factory engine binary is unavailable. */
export function inProcessFloor(toolCall: AcpToolCall, cwd: string): AcpGateVerdict {
  const payload = acpToolToHookPayload(toolCall, cwd)
  const command = payload.tool_name === "Bash" ? String((payload.tool_input as { command?: string }).command ?? "") : ""
  const hit = command ? Catastrophic.check(command) : undefined
  return hit
    ? { allow: false, reason: `hard floor: ${hit.reason}`, fallback: true }
    : { allow: true, fallback: true }
}

/** Map the primary ACP target. Admission must use acpToolToHookPayloads below. */
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

/** The hook understands one Write path per request, so gate every ACP target. */
export function acpToolToHookPayloads(toolCall: AcpToolCall, cwd: string, sessionId?: string) {
  const primary = acpToolToHookPayload(toolCall, cwd, sessionId)
  if (primary.tool_name !== "Write") return [primary]
  const raw = (toolCall.rawInput && typeof toolCall.rawInput === "object" ? toolCall.rawInput : {}) as Record<string, unknown>
  const paths = new Set<string>()
  const add = (path: unknown) => {
    if (typeof path === "string" && path.trim()) paths.add(path)
  }
  add((primary.tool_input as { file_path?: string }).file_path)
  for (const key of ["file_path", "path", "source", "source_path", "destination", "destination_path", "old_path", "new_path", "from", "to"]) {
    add(raw[key])
  }
  if (Array.isArray(toolCall.locations)) {
    for (const location of toolCall.locations) {
      add(typeof location === "string" ? location : location?.path)
    }
  }
  return [...paths].map((file_path) => ({ ...primary, tool_input: { file_path } }))
}

/**
 * The Factory engine binary (`allternit-factory`), found the same way every
 * other gizzi engine call finds it: next to gizzi, `$ALLTERNIT_FACTORY_BIN`,
 * `~/.allternit/bin`, then PATH.
 */
export function findEngineBin(env: NodeJS.ProcessEnv = process.env): string | undefined {
  return locateEngine({ env })?.path
}

/** The gate hook is the engine's maintenance CLI: `allternit-factory internal core …`. */
export const ENGINE_CORE_ARGV = ["internal", "core"] as const

/** Only a successful hook can allow; successful silence allows, `deny` denies. */
export function parseHookOutput(stdout: string, exitCode: number | null): AcpGateVerdict {
  if (exitCode !== 0) {
    return { allow: false, reason: exitCode === null ? "gate terminated without a successful exit (fail closed)" : `gate exited ${exitCode} (fail closed)` }
  }
  const text = stdout.trim()
  if (!text) return { allow: true }
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
 * Decide one ACP tool call, never prompting. Order: plan-mode read-only
 * (deny write/exec first), then the Factory gate; with no engine binary,
 * fail closed for WIH-bound or effectful calls. Only unbound read-only calls
 * can use gizzi's in-process catastrophic floor as a fallback.
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
  /** Session permission mode and the tool's permission class ("read" | "edit" | "bash"). */
  mode?: string
  permission?: string
}): Promise<AcpGateVerdict> {
  if (opts.permission && planModeDenies(opts.mode, opts.permission)) {
    return { allow: false, reason: "plan mode is read-only" }
  }
  const bin = opts.bin ?? findEngineBin()
  if (!bin) {
    const floor = inProcessFloor(opts.toolCall, opts.cwd)
    if (!floor.allow) return floor
    if (opts.wihId) {
      // Same default receipt root and marker-presence semantics as the engine gate.
      // A marker is authoritative even if its JSON is empty or unreadable.
      const validId = !/[\/\\\0]/.test(opts.wihId) && !opts.wihId.startsWith(".")
      const marker = validId && join(opts.root ?? opts.cwd, ".allternit", "receipts", "_replay", `run_${opts.wihId}.json`)
      if (marker && existsSync(marker)) {
        return { allow: false, reason: `replay: recorded result served by the gate (run run_${opts.wihId} is replaying, effects: recorded_only)`, fallback: true }
      }
      return { allow: false, reason: `Allternit gate unavailable: cannot enforce WIH ${opts.wihId} policy and lease coverage (fail closed)`, fallback: true }
    }
    const payload = acpToolToHookPayload(opts.toolCall, opts.cwd)
    const readOnly = ["Read", "WebFetch"].includes(payload.tool_name) && (!opts.permission || opts.permission === "read")
    if (!readOnly) return { allow: false, reason: "Allternit gate unavailable: effectful or unknown tool call cannot be authorized (fail closed)", fallback: true }
    return floor
  }
  const args = [...ENGINE_CORE_ARGV, "--root", opts.root ?? opts.cwd, "hook", "claude-pretool", "--harness", opts.harness, "--workspace", opts.cwd]
  if (opts.wihId) args.push("--wih", opts.wihId)
  const payloads = acpToolToHookPayloads(opts.toolCall, opts.cwd, opts.sessionId)
  if (!payloads.length) return { allow: false, reason: "cannot recover write paths for ACP mutation (fail closed)" }
  if (opts.toolCall.kind === "move" && payloads.length < 2) {
    return { allow: false, reason: "cannot recover both source and destination write paths for ACP move (fail closed)" }
  }
  // All targets must pass within the original per-call timeout budget.
  const deadline = Date.now() + (opts.timeoutMs ?? 30_000)
  for (const payload of payloads) {
    const remaining = deadline - Date.now()
    if (remaining <= 0) return { allow: false, reason: "gate timed out (fail closed)" }
    const verdict = await runHook(bin, args, JSON.stringify(payload), remaining)
    if (!verdict.allow) return verdict
  }
  return { allow: true }
}

function runHook(bin: string, args: string[], payload: string, timeoutMs: number): Promise<AcpGateVerdict> {
  return new Promise<AcpGateVerdict>((resolve) => {
    const child = spawn(bin, args, { stdio: ["pipe", "pipe", "ignore"] })
    let stdout = ""
    const timer = setTimeout(() => {
      child.kill()
      resolve({ allow: false, reason: "gate timed out (fail closed)" })
    }, timeoutMs)
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
