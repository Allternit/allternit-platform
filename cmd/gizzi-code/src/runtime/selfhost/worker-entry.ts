/**
 * Process wiring for `gizzi worker` and `gizzi worker attach`: env and flag
 * resolution, the self-hosted offline switch, JSON logs, SIGTERM/SIGINT.
 * Loaded by src/cli/commands/worker.ts through a relative dynamic import.
 */
import fs from "node:fs/promises"
import path from "node:path"
import { makeJsonLogger } from "@/runtime/fabric-transport/daemon-util"
import { installSelfHostedGuard } from "@/shared/selfhost/offline"
import { createGizziToolExecutor, DEFAULT_WORKDIR } from "./executor"
import { runAttach, runSelfHostWorker } from "./worker"
import type { EnvironmentPolicy } from "./egress"

export interface WorkerArgs {
  server?: string
  environmentKey?: string
  environment?: string
  workerId?: string
  blockMs?: number
  drain?: boolean
  reclaimOlderThanMs?: number
  autoStop?: number
  onWork?: string
  workdir?: string
}

const env = (name: string) => process.env[name]?.trim() || undefined

function setup() {
  // A self-hosted worker never talks to Allternit.
  process.env.ALLTERNIT_SELF_HOSTED ||= "1"
  installSelfHostedGuard()
  // This process runs tools; it never forwards them.
  delete process.env.GIZZI_TOOL_HOST
  const log = makeJsonLogger()
  const stop = new AbortController()
  const onSignal = (signal: string) => {
    if (stop.signal.aborted) {
      log("warn", "worker.force_exit", { signal })
      process.exit(1)
    }
    log("info", "worker.shutdown_requested", { signal })
    stop.abort("stopping")
  }
  process.on("SIGTERM", () => onSignal("SIGTERM"))
  process.on("SIGINT", () => onSignal("SIGINT"))
  return { log, stop }
}

async function ensureWorkdir(dir: string): Promise<string> {
  const resolved = path.resolve(dir)
  await fs.mkdir(resolved, { recursive: true })
  return resolved
}

function serverUrl(raw: string | undefined): string {
  const value = (raw ?? "").trim()
  if (!value) throw new Error("--server (or ALLTERNIT_SERVER_URL) is required")
  const url = new URL(value)
  if (url.protocol !== "https:" && url.protocol !== "http:") throw new Error(`--server must be an http(s) URL, got ${url.protocol}`)
  return value.replace(/\/+$/, "")
}

export async function runWorkerCommand(args: WorkerArgs): Promise<number> {
  const { log, stop } = setup()
  try {
    const server = serverUrl(args.server ?? env("ALLTERNIT_SERVER_URL"))
    const environmentKey = args.environmentKey ?? env("ALLTERNIT_ENVIRONMENT_KEY")
    const environmentId = args.environment ?? env("ALLTERNIT_ENVIRONMENT_ID")
    if (!environmentKey) throw new Error("--environment-key (or ALLTERNIT_ENVIRONMENT_KEY) is required")
    if (!environmentKey.startsWith("aek_")) throw new Error("--environment-key must be an environment key (aek_...)")
    if (!environmentId) throw new Error("--environment (or ALLTERNIT_ENVIRONMENT_ID) is required")
    const workdir = await ensureWorkdir(args.workdir ?? env("ALLTERNIT_WORKDIR") ?? DEFAULT_WORKDIR)
    return await runSelfHostWorker({
      server,
      environmentKey,
      environmentId,
      workerId: args.workerId ?? env("ALLTERNIT_WORKER_ID"),
      blockMs: args.blockMs,
      drain: args.drain === true,
      reclaimOlderThanMs: args.reclaimOlderThanMs,
      autoStopSecs: args.autoStop && args.autoStop > 0 ? args.autoStop : undefined,
      onWork: args.onWork || undefined,
      workdir,
      log,
      stop: stop.signal,
      executor: createGizziToolExecutor({ workdir, log }),
    })
  } catch (error) {
    log("error", "worker.fatal", { error: (error as Error).message })
    return 1
  }
}

export async function runAttachCommand(args: Pick<WorkerArgs, "server" | "workdir" | "blockMs">): Promise<number> {
  const { log, stop } = setup()
  try {
    const server = serverUrl(args.server ?? env("ALLTERNIT_SERVER_URL"))
    const workId = env("ALLTERNIT_WORK_ID") ?? env("WORK_ID")
    const sessionId = env("ALLTERNIT_SESSION_ID") ?? env("SESSION_ID")
    const secret = env("ALLTERNIT_WORK_SECRET")
    if (!workId || !sessionId || !secret) {
      throw new Error("ALLTERNIT_WORK_ID, ALLTERNIT_SESSION_ID and ALLTERNIT_WORK_SECRET must be set (gizzi worker --on-work sets them)")
    }
    // Keep the secret out of every child this process starts.
    delete process.env.ALLTERNIT_WORK_SECRET
    let policy: EnvironmentPolicy = { network: "none" }
    const rawPolicy = env("ALLTERNIT_ENVIRONMENT_POLICY")
    if (rawPolicy) {
      try {
        policy = JSON.parse(rawPolicy) as EnvironmentPolicy
      } catch {
        log("warn", "worker.attach_policy_unreadable", { note: "using network: none" })
      }
    }
    const workdir = await ensureWorkdir(args.workdir ?? env("ALLTERNIT_WORKDIR") ?? DEFAULT_WORKDIR)
    return await runAttach({
      server,
      workId,
      sessionId,
      secret,
      environmentId: env("ALLTERNIT_ENVIRONMENT_ID"),
      policy,
      blockMs: args.blockMs,
      log,
      stop: stop.signal,
      executor: createGizziToolExecutor({ workdir, log }),
    })
  } catch (error) {
    log("error", "worker.fatal", { error: (error as Error).message })
    return 1
  }
}
