/**
 * `gizzi worker`: the self-hosted Agents Server's sandbox worker (plan step 10).
 *
 * Loop (docs/SELFHOST_WORKER_PROTOCOL.md):
 *   claim (environment key, long-poll block_ms)
 *     → renew the lease every 20 s; 409 lease_stale stops the item at once
 *       and nothing more is posted for it
 *     → long-poll tool_calls/next with the session secret, run each call
 *       with gizzi's own tools, post the result (truncated to 100 KiB)
 *     → complete the item when tool_calls/next answers 204 with
 *       `X-Session-Closed: true`
 *   SIGTERM → release the item (it is ready again at once) and exit 0.
 *
 * Modes: always-on (default), `--drain` (one item, then exit 0; exits 0 after
 * one empty claim too), `--on-work <script>` (run the script per item while
 * this process keeps the lease; the script typically starts a sandbox that
 * runs `gizzi worker attach`), `--auto-stop <secs>` (exit 0 after that long
 * with no work).
 *
 * The secret and the environment key are never logged.
 */
import { spawn } from "node:child_process"
import os from "node:os"
import { createBackoff, type DaemonLog } from "@/runtime/fabric-transport/daemon-util"
import { createWorkerClient, SelfHostHttpError, type WorkClaim, type WorkerClient } from "./client"
import type { EnvironmentPolicy } from "./egress"
import type { ToolExecutor, ToolResult } from "./executor"

export const MAX_RESULT_BYTES = 102_400
export const RENEW_INTERVAL_MS = 20_000
const RETRY_RESULT_BYTES = 50 * 1024
const RESULT_POST_ATTEMPTS = 5
const DONE_CACHE_SIZE = 512

/** Cut `output` to at most `max` UTF-8 bytes, with a note saying so. */
export function truncateResult(output: string, max = MAX_RESULT_BYTES): string {
  const bytes = Buffer.byteLength(output, "utf8")
  if (bytes <= max) return output
  const note = `\n\n[gizzi worker: output truncated from ${bytes} bytes to fit the ${Math.floor(max / 1024)} KiB result limit]`
  const budget = Math.max(0, max - Buffer.byteLength(note, "utf8"))
  let text = Buffer.from(output, "utf8").subarray(0, budget).toString("utf8")
  // A cut inside a multi-byte character decodes to U+FFFD; drop it.
  while (text.endsWith("�") && Buffer.byteLength(text, "utf8") > 0) text = text.slice(0, -1)
  return text + note
}

export function defaultWorkerId(): string {
  return `${os.hostname()}-${process.pid}`
}

type ItemOutcome = "closed" | "idle" | "stale" | "stopped" | "failed"

interface ServeInput {
  client: WorkerClient
  executor: ToolExecutor
  workId: string
  sessionId: string
  secret: string
  blockMs: number
  signal: AbortSignal
  log: DaemonLog
  /** Drain mode: give the item back after the session goes quiet. */
  drain?: boolean
  /** Results already produced by this process, by tool_call_id (run each id once). */
  done?: Map<string, ToolResult>
}

function remember(done: Map<string, ToolResult>, id: string, result: ToolResult) {
  done.set(id, result)
  if (done.size > DONE_CACHE_SIZE) done.delete(done.keys().next().value as string)
}

async function postResult(input: ServeInput, toolCallId: string, result: ToolResult): Promise<"ok" | "stale" | "stopped"> {
  const backoff = createBackoff({ startMs: 500, maxMs: 8000 })
  let output = truncateResult(result.output)
  for (let attempt = 1; attempt <= RESULT_POST_ATTEMPTS; attempt++) {
    if (input.signal.aborted) return "stopped"
    try {
      await input.client.submitResult(toolCallId, input.secret, output, result.is_error)
      return "ok"
    } catch (error) {
      if (error instanceof SelfHostHttpError) {
        if (error.status === 413) {
          output = truncateResult(result.output, RETRY_RESULT_BYTES)
          continue
        }
        if (error.leaseLost) return "stale"
        if (error.status < 500) {
          input.log("error", "worker.result_rejected", { tool_call_id: toolCallId, status: error.status, code: error.code })
          return "ok"
        }
      }
      input.log("warn", "worker.result_post_failed", { tool_call_id: toolCallId, attempt, error: (error as Error).message })
      await backoff.wait()
    }
  }
  input.log("error", "worker.result_dropped", { tool_call_id: toolCallId })
  return "ok"
}

/**
 * Serve one session's tool calls until it is closed, the lease is lost, or
 * `signal` aborts. Shared by the worker loop and `gizzi worker attach`.
 */
export async function serveToolCalls(input: ServeInput): Promise<"closed" | "idle" | "stale" | "stopped"> {
  const done = input.done ?? new Map<string, ToolResult>()
  const backoff = createBackoff()
  let ranAny = false
  let emptyPolls = 0
  while (!input.signal.aborted) {
    let next
    try {
      next = await input.client.nextToolCall(input.workId, input.secret, input.blockMs, input.signal)
      backoff.reset()
    } catch (error) {
      if (input.signal.aborted) break
      if (error instanceof SelfHostHttpError) {
        if (error.leaseLost) return "stale"
        if (error.status === 404 || error.status === 410) return "closed"
      }
      input.log("warn", "worker.next_failed", { work_id: input.workId, error: (error as Error).message })
      await backoff.wait()
      continue
    }
    if (next.kind === "closed") return "closed"
    if (next.kind === "idle") {
      emptyPolls++
      if (input.drain && (ranAny || emptyPolls >= 2)) return "idle"
      continue
    }
    emptyPolls = 0
    ranAny = true
    const call = next.call
    let result = done.get(call.tool_call_id)
    if (result) {
      input.log("info", "worker.tool_call_repeat", { tool_call_id: call.tool_call_id, name: call.name })
    } else {
      const started = Date.now()
      input.log("info", "worker.tool_call_started", { tool_call_id: call.tool_call_id, name: call.name })
      result = await input.executor.run(input.sessionId, call, input.signal)
      // Lease lost or shutting down mid-call: discard, post nothing.
      if (input.signal.aborted) break
      remember(done, call.tool_call_id, result)
      input.log("info", "worker.tool_call_finished", {
        tool_call_id: call.tool_call_id,
        name: call.name,
        is_error: result.is_error,
        bytes: Buffer.byteLength(result.output, "utf8"),
        ms: Date.now() - started,
      })
    }
    const posted = await postResult(input, call.tool_call_id, result)
    if (posted === "stale") return "stale"
    if (posted === "stopped") break
  }
  return input.signal.reason === "stale" ? "stale" : "stopped"
}

export interface SelfHostWorkerOptions {
  server: string
  environmentKey: string
  environmentId: string
  workerId?: string
  blockMs?: number
  drain?: boolean
  reclaimOlderThanMs?: number
  /** Exit 0 after this many seconds without a claim. */
  autoStopSecs?: number
  /** Run this command per claimed item instead of serving tool calls here. */
  onWork?: string
  workdir: string
  log: DaemonLog
  /** Aborts on SIGTERM/SIGINT. */
  stop: AbortSignal
  executor: ToolExecutor
  fetchImpl?: typeof fetch
}

/** Keeps the lease alive; calls onStale once when the lease is gone. */
function keepLease(client: WorkerClient, claim: WorkClaim, log: DaemonLog, onStale: () => void) {
  let expiresAt = Date.parse(claim.lease_expires_at) || Date.now() + 60_000
  let stopped = false
  const timer = setInterval(async () => {
    if (stopped) return
    try {
      const lease = await client.renew(claim.work_id, claim.lease_generation)
      if (lease.lease_expires_at) expiresAt = Date.parse(lease.lease_expires_at) || Date.now() + 60_000
    } catch (error) {
      if (stopped) return
      if (error instanceof SelfHostHttpError && error.status === 409) {
        log("warn", "worker.lease_stale", { work_id: claim.work_id, lease_generation: claim.lease_generation })
        stopped = true
        onStale()
        return
      }
      log("warn", "worker.renew_failed", { work_id: claim.work_id, error: (error as Error).message })
      if (Date.now() >= expiresAt) {
        log("warn", "worker.lease_expired", { work_id: claim.work_id })
        stopped = true
        onStale()
      }
    }
  }, RENEW_INTERVAL_MS)
  return {
    stop() {
      stopped = true
      clearInterval(timer)
    },
  }
}

/** Env for an --on-work script. Leaves out the environment key; carries the session secret for `gizzi worker attach`. */
export function onWorkEnv(opts: Pick<SelfHostWorkerOptions, "server" | "environmentId">, claim: WorkClaim): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = { ...process.env }
  delete env.ALLTERNIT_ENVIRONMENT_KEY
  delete env.GIZZI_TOOL_HOST_TOKEN
  return {
    ...env,
    ALLTERNIT_SERVER_URL: opts.server,
    ALLTERNIT_ENVIRONMENT_ID: opts.environmentId,
    ALLTERNIT_WORK_ID: claim.work_id,
    ALLTERNIT_SESSION_ID: claim.session_id,
    ALLTERNIT_WORK_SECRET: claim.session_secret,
    ALLTERNIT_LEASE_GENERATION: String(claim.lease_generation),
    ALLTERNIT_ENVIRONMENT_POLICY: JSON.stringify(claim.environment ?? { network: "none" }),
    ALLTERNIT_SELF_HOSTED: "1",
    // Short names from the protocol doc.
    WORK_ID: claim.work_id,
    SESSION_ID: claim.session_id,
    LEASE_GENERATION: String(claim.lease_generation),
  }
}

function runOnWork(
  opts: SelfHostWorkerOptions,
  claim: WorkClaim,
  signal: AbortSignal,
): Promise<{ outcome: "closed" | "failed" | "stopped"; message?: string }> {
  return new Promise((resolve) => {
    opts.log("info", "worker.on_work_started", { work_id: claim.work_id, session_id: claim.session_id })
    const child = spawn("/bin/sh", ["-c", opts.onWork!], {
      cwd: opts.workdir,
      env: onWorkEnv(opts, claim),
      stdio: ["ignore", "inherit", "inherit"],
    })
    let killTimer: ReturnType<typeof setTimeout> | undefined
    const onAbort = () => {
      child.kill("SIGTERM")
      killTimer = setTimeout(() => child.kill("SIGKILL"), 10_000)
      killTimer.unref?.()
    }
    if (signal.aborted) onAbort()
    else signal.addEventListener("abort", onAbort, { once: true })
    child.once("error", (error) => {
      signal.removeEventListener("abort", onAbort)
      resolve({ outcome: "failed", message: `on-work script failed to start: ${error.message}` })
    })
    child.once("exit", (code, sig) => {
      signal.removeEventListener("abort", onAbort)
      if (killTimer) clearTimeout(killTimer)
      opts.log(code === 0 ? "info" : "warn", "worker.on_work_exited", { work_id: claim.work_id, code, signal: sig })
      if (signal.aborted) return resolve({ outcome: "stopped" })
      if (code === 0) return resolve({ outcome: "closed" })
      resolve({ outcome: "failed", message: `on-work script exited with ${sig ? `signal ${sig}` : `code ${code}`}` })
    })
  })
}

async function serveItem(
  opts: SelfHostWorkerOptions,
  client: WorkerClient,
  claim: WorkClaim,
  done: Map<string, ToolResult>,
): Promise<ItemOutcome> {
  const item = new AbortController()
  const onStop = () => item.abort("stopping")
  if (opts.stop.aborted) onStop()
  else opts.stop.addEventListener("abort", onStop, { once: true })
  const lease = keepLease(client, claim, opts.log, () => item.abort("stale"))

  let outcome: ItemOutcome
  let failure: string | undefined
  try {
    if (opts.onWork) {
      const ran = await runOnWork(opts, claim, item.signal)
      outcome = ran.outcome
      failure = ran.message
    } else {
      try {
        await opts.executor.prepare({
          sessionId: claim.session_id,
          workId: claim.work_id,
          environmentId: opts.environmentId,
          policy: claim.environment ?? { network: "none" },
        })
      } catch (error) {
        failure = `worker could not prepare the sandbox: ${(error as Error).message}`
        opts.log("error", "worker.prepare_failed", { work_id: claim.work_id, error: (error as Error).message })
        outcome = "failed"
        throw error
      }
      outcome = await serveToolCalls({
        client,
        executor: opts.executor,
        workId: claim.work_id,
        sessionId: claim.session_id,
        secret: claim.session_secret,
        blockMs: opts.blockMs ?? 25_000,
        signal: item.signal,
        log: opts.log,
        drain: opts.drain,
        done,
      })
    }
  } catch {
    outcome = "failed"
  } finally {
    lease.stop()
    opts.stop.removeEventListener("abort", onStop)
  }

  if (item.signal.reason === "stale" || outcome === "stale") {
    // Another worker owns the item now: post nothing, complete nothing, and
    // stop this worker's leftovers for the session (background tasks, proxy).
    if (!opts.onWork) await opts.executor.finish(claim.session_id)
    return "stale"
  }

  const settle = async (what: "complete" | "release", fn: () => Promise<unknown>) => {
    try {
      await fn()
      opts.log("info", what === "complete" ? "worker.completed" : "worker.released", {
        work_id: claim.work_id,
        outcome,
      })
    } catch (error) {
      opts.log("warn", `worker.${what}_failed`, { work_id: claim.work_id, error: (error as Error).message })
    }
  }

  if (opts.stop.aborted || outcome === "stopped") {
    await settle("release", () => client.release(claim.work_id, claim.lease_generation))
    return "stopped"
  }
  if (outcome === "idle") {
    await settle("release", () => client.release(claim.work_id, claim.lease_generation))
    return "idle"
  }
  if (outcome === "failed") {
    await settle("complete", () => client.complete(claim.work_id, claim.lease_generation, "failed", failure))
  } else {
    await settle("complete", () => client.complete(claim.work_id, claim.lease_generation, "success"))
  }
  if (!opts.onWork) await opts.executor.finish(claim.session_id)
  return outcome
}

/** The always-on / drain / on-work loop. Resolves with the process exit code. */
export async function runSelfHostWorker(opts: SelfHostWorkerOptions): Promise<number> {
  const client = createWorkerClient({ server: opts.server, environmentKey: opts.environmentKey, fetchImpl: opts.fetchImpl })
  const workerId = opts.workerId || defaultWorkerId()
  const backoff = createBackoff()
  const done = new Map<string, ToolResult>()
  let idleSince = Date.now()

  opts.log("info", "worker.start", {
    server: client.base,
    environment_id: opts.environmentId,
    worker_id: workerId,
    mode: opts.onWork ? "on-work" : opts.drain ? "drain" : "always-on",
    workdir: opts.workdir,
    block_ms: opts.blockMs ?? 25_000,
    auto_stop_secs: opts.autoStopSecs ?? null,
  })

  while (true) {
    if (opts.stop.aborted) {
      opts.log("info", "worker.stopping", { reason: "shutdown signal" })
      return 0
    }
    let claim: WorkClaim | null
    try {
      claim = await client.claim(
        opts.environmentId,
        {
          worker_id: workerId,
          block_ms: Math.max(0, Math.min(30_000, opts.blockMs ?? 25_000)),
          ...(opts.reclaimOlderThanMs !== undefined ? { reclaim_older_than_ms: opts.reclaimOlderThanMs } : {}),
        },
        opts.stop,
      )
      backoff.reset()
    } catch (error) {
      if (opts.stop.aborted) continue
      if (error instanceof SelfHostHttpError && (error.status === 401 || error.status === 403)) {
        opts.log("error", "worker.unauthorized", {
          status: error.status,
          code: error.code,
          hint: "check --environment-key and --environment (the key must belong to this environment)",
        })
        return 1
      }
      opts.log("warn", "worker.claim_failed", { error: (error as Error).message, backoff_ms: backoff.currentMs })
      await backoff.wait()
      continue
    }

    if (!claim) {
      if (opts.drain) {
        opts.log("info", "worker.drain_no_work", {})
        return 0
      }
      if (opts.autoStopSecs && Date.now() - idleSince >= opts.autoStopSecs * 1000) {
        opts.log("info", "worker.auto_stop", { idle_secs: Math.round((Date.now() - idleSince) / 1000) })
        return 0
      }
      continue
    }

    opts.log("info", "worker.claimed", {
      work_id: claim.work_id,
      session_id: claim.session_id,
      lease_generation: claim.lease_generation,
      network: (claim.environment as EnvironmentPolicy | undefined)?.network ?? "none",
    })
    const outcome = await serveItem(opts, client, claim, done)
    idleSince = Date.now()
    if (outcome === "stopped") return 0
    if (opts.drain) return outcome === "failed" ? 1 : 0
  }
}

/** `gizzi worker attach`: serve one already-claimed item from env (set by --on-work). */
export interface AttachOptions {
  server: string
  workId: string
  sessionId: string
  secret: string
  environmentId?: string
  policy: EnvironmentPolicy
  blockMs?: number
  log: DaemonLog
  stop: AbortSignal
  executor: ToolExecutor
  fetchImpl?: typeof fetch
}

export async function runAttach(opts: AttachOptions): Promise<number> {
  const client = createWorkerClient({ server: opts.server, fetchImpl: opts.fetchImpl })
  opts.log("info", "worker.attach_start", { server: client.base, work_id: opts.workId, session_id: opts.sessionId })
  await opts.executor.prepare({
    sessionId: opts.sessionId,
    workId: opts.workId,
    environmentId: opts.environmentId,
    policy: opts.policy,
  })
  const outcome = await serveToolCalls({
    client,
    executor: opts.executor,
    workId: opts.workId,
    sessionId: opts.sessionId,
    secret: opts.secret,
    blockMs: opts.blockMs ?? 25_000,
    signal: opts.stop,
    log: opts.log,
  })
  await opts.executor.finish(opts.sessionId)
  opts.log("info", "worker.attach_done", { work_id: opts.workId, outcome })
  // 0: session closed or asked to stop. 3: the lease moved on; the parent must not complete.
  return outcome === "stale" ? 3 : 0
}
