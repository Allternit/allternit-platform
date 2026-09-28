/**
 * Fabric Transport worker client for Gizzi (A-T4; GIZZI_WORKER_SPEC §5).
 *
 * Gizzi authenticates as `a://workspace/{ws}/principal/gizzi` with its
 * provisioned bearer token, long-polls the claim endpoint, executes the
 * job's deterministic shell/code steps, heartbeats, checkpoints at
 * committed boundaries, and completes with the typed Result envelope.
 *
 * Execution posture (P-T2 compute placement):
 *   - `GIZZI_COMPUTE_MODE=local` (default): `Sandbox.wrap` — bubblewrap on
 *     Linux / sandbox-exec on macOS (falls back to an unsandboxed shell,
 *     loudly logged, when no driver exists).
 *   - `GIZZI_COMPUTE_MODE=vm`: steps run inside the Lima VM via
 *     `executeInVM` (the repo's current VM machinery — `limactl shell
 *     allternit`; Lima runs on macOS and Linux hosts). The operator must
 *     also declare `compute.vm` on the worker's principal so placement
 *     (§8.8) routes vm-required jobs here:
 *       PUT /api/v1/fabric/transport/principals/<id>/capabilities
 *         { "capabilities": [..., "compute.vm"] }
 *     Identity/attribution are unchanged: the executor remains
 *     `a://workspace/{ws}/principal/gizzi` wherever the steps run.
 *   - `GIZZI_COMPUTE_MODE=cloud`: always-on worker as
 *     `a://workspace/{ws}/principal/gizzi-cloud` (`compute.cloud`). Runs
 *     on the host that stays up when the laptop sleeps. Does not see
 *     laptop trusted folders — uses ALLTERNIT_CLOUD_WORKSPACE.
 *
 * Operator flow for the token (provisioned once, V162):
 *   POST /api/v1/fabric/transport/principals/<url-encoded principal>/provision-token
 * then start the worker with:
 *   ALLTERNIT_GIZZI_TOKEN=atok_… bun src/runtime/fabric-transport/worker-entry.ts
 * (or ALLTERNIT_GIZZI_TOKEN_FILE=/path/to/file for mode-0600 token files).
 */

import { spawn } from "node:child_process"
import { Sandbox } from "../integrations/shell/sandbox"
import { executeInVM } from "../vm"

const API = (process.env.ALLTERNIT_API_URL ?? "http://127.0.0.1:8013").replace(/\/+$/, "")
const TOKEN = process.env.ALLTERNIT_GIZZI_TOKEN ?? readTokenFile()
const LEASE_SECS = Number(process.env.ALLTERNIT_GIZZI_LEASE_SECS ?? "60")
const HEARTBEAT_MS = Math.max(1000, Math.floor((LEASE_SECS / 3) * 1000))
const COMPUTE_MODE = process.env.GIZZI_COMPUTE_MODE ?? "local"

function readTokenFile(): string | null {
  const path = process.env.ALLTERNIT_GIZZI_TOKEN_FILE
  if (!path) return null
  try {
    // Lazy require keeps bun/node parity.
    return require("node:fs").readFileSync(path, "utf8").trim() as string
  } catch {
    return null
  }
}

interface LeaseGrant {
  job_id: string
  run_id: string
  lease_id: string
  lease_generation: number
  lease_expires_at: string
  payload: { steps?: string[]; [k: string]: unknown }
  current_checkpoint_id?: string | null
}

async function api<T>(path: string, init: RequestInit = {}): Promise<T> {
  const headers = new Headers(init.headers)
  headers.set("Content-Type", "application/json")
  if (TOKEN) headers.set("Authorization", `Bearer ${TOKEN}`)
  const res = await fetch(`${API}/api/v1${path}`, { ...init, headers })
  if (!res.ok) {
    const body = await res.text().catch(() => "")
    throw new Error(`${res.status} ${path}: ${body.slice(0, 300)}`)
  }
  return (await res.json()) as T
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

async function runStepLocal(command: string, cwd: string, sessionId: string): Promise<{ code: number; output: string }> {
  const policy = {
    allowWritePaths: [] as string[],
    allowNetwork: false,
    allowedDomains: [] as string[],
  }
  // Existing sandbox posture: bwrap (Linux) / sandbox-exec (macOS); null when
  // no driver is available — then we run unsandboxed but never silently claim
  // otherwise (Sandbox.wrap logs loudly).
  const wrapped = await Sandbox.wrap({ command, shell: "/bin/bash", cwd, sessionID: sessionId, policy })
  const bin = wrapped?.bin ?? "/bin/bash"
  const args = wrapped?.args ?? ["-c", command]
  return new Promise((resolve) => {
    const child = spawn(bin, args, { cwd })
    let output = ""
    child.stdout.on("data", (d) => (output += String(d)))
    child.stderr.on("data", (d) => (output += String(d)))
    child.on("close", (code) => resolve({ code: code ?? 1, output: output.slice(-4000) }))
    child.on("error", (err) => resolve({ code: 1, output: String(err) }))
  })
}

async function runStep(command: string, cwd: string, sessionId: string): Promise<{ code: number; output: string }> {
  if (COMPUTE_MODE !== "vm") {
    return runStepLocal(command, cwd, sessionId)
  }
  // VM mode (P-T2): run the step inside the Lima VM. The working directory is
  // interpreted inside the guest; jobs aimed at a VM worker should use guest
  // paths (the VM mounts the host home by default via allternit.yaml).
  try {
    const result = await executeInVM("bash", ["-c", command], { workingDir: cwd })
    return { code: result.exitCode, output: (result.stdout + result.stderr).slice(-4000) }
  } catch (err) {
    return { code: 1, output: `vm step failed: ${String(err)}`.slice(-4000) }
  }
}

export interface FabricWorkerDaemonHooks {
  /** Structured log sink (daemon mode). `event` is a stable snake_case name. */
  log: (level: "info" | "warn" | "error", event: string, fields?: Record<string, unknown>) => void
  /** Graceful-stop check, evaluated before each claim. */
  shouldStop: () => boolean
}

export interface RunFabricWorkerOptions {
  cwd?: string
  /** Daemon mode (P-T3): exponential claim backoff, structured logs,
   *  graceful stop between jobs. Claim protocol unchanged. */
  daemon?: FabricWorkerDaemonHooks
}

export async function runFabricWorker(opts: RunFabricWorkerOptions = {}): Promise<void> {
  const cwd = opts.cwd ?? process.cwd()
  const daemon = opts.daemon
  const log = (level: "info" | "warn" | "error", event: string, fields?: Record<string, unknown>) => {
    if (daemon) {
      daemon.log(level, event, fields)
    } else {
      const line = `[gizzi-worker] ${event}${fields ? " " + JSON.stringify(fields) : ""}`
      if (level === "error") console.error(line)
      else if (level === "warn") console.warn(line)
      else console.log(line)
    }
  }
  if (!TOKEN) {
    throw new Error(
      "ALLTERNIT_GIZZI_TOKEN (or ALLTERNIT_GIZZI_TOKEN_FILE) is required — provision it via " +
        "POST /api/v1/fabric/transport/principals/<principal>/provision-token",
    )
  }
  if (COMPUTE_MODE === "vm") {
    log("info", "worker.vm_mode", {
      note: "steps run inside the Lima VM; declare compute.vm on this worker's principal " +
        "(PUT /fabric/transport/principals/<id>/capabilities) so vm-required jobs route here",
    })
  }

  // Daemon backoff state (reset on every successful claim).
  let backoffMs = 1000
  const BACKOFF_MAX_MS = 30_000

  while (true) {
    if (daemon?.shouldStop()) {
      log("info", "worker.stopping", { reason: "shutdown signal" })
      return
    }
    let grant: LeaseGrant
    try {
      grant = await api<LeaseGrant>("/fabric/transport/claim", {
        method: "POST",
        body: JSON.stringify({ wait_secs: 25, lease_ttl_secs: LEASE_SECS }),
      })
      backoffMs = 1000
    } catch (e) {
      log("warn", "worker.claim_failed", { error: (e as Error).message, backoff_ms: daemon ? backoffMs : 2000 })
      await sleep(daemon ? backoffMs + Math.floor(Math.random() * 250) : 2000)
      if (daemon) backoffMs = Math.min(backoffMs * 2, BACKOFF_MAX_MS)
      continue
    }

    log("info", "worker.claimed", { job_id: grant.job_id, lease_generation: grant.lease_generation })
    const steps: string[] = Array.isArray(grant.payload?.steps) ? (grant.payload.steps as string[]) : []
    const agentic = grant.payload?.agentic as
      | { task?: string; model?: string; system?: string; bot_id?: string; max_steps?: number; max_tokens?: number }
      | undefined
    const stepResults: Array<{ step: number; code: number }> = []
    let heartbeat: ReturnType<typeof setInterval> | null = null
    let failed = false

    try {
      heartbeat = setInterval(() => {
        api(`/fabric/transport/jobs/${grant.job_id}/heartbeat`, {
          method: "POST",
          body: JSON.stringify({ lease_id: grant.lease_id, lease_generation: grant.lease_generation }),
        }).catch(() => {})
      }, HEARTBEAT_MS)

      if (agentic) {
        // Agentic job kind (P2.2): bounded model-agent loop through the
        // existing model router, checkpointed per committed step.
        const { runAgenticLoop } = await import("./agentic")
        await runAgenticLoop(agentic.task ?? String(grant.payload?.message ?? ""), {
          apiBase: API,
          operatorKey: process.env.ALLTERNIT_OPERATOR_API_KEY ?? null,
          model: agentic.model ?? process.env.ALLTERNIT_AGENTIC_MODEL ?? "openai/gpt-4o-mini",
          grants: (await import("./agentic")).parseGrants(
            COMPUTE_MODE === "cloud"
              ? (process.env.ALLTERNIT_CLOUD_WORKSPACE
                  ? JSON.stringify([process.env.ALLTERNIT_CLOUD_WORKSPACE])
                  : "[]")
              : process.env.ALLTERNIT_WORKER_TRUSTED_FOLDERS,
          ),
          checkpoint: (stepIndex, cursor) =>
            api(`/runs/${grant.run_id}/checkpoints`, {
              method: "POST",
              body: JSON.stringify({ step_index: stepIndex, cursor_state: cursor }),
            }).then(() => {}).catch(() => {}),
          complete: (success, summary, outputs) =>
            api(`/fabric/transport/jobs/${grant.job_id}/complete`, {
              method: "POST",
              body: JSON.stringify({
                lease_id: grant.lease_id,
                lease_generation: grant.lease_generation,
                success,
                summary,
                outputs: { worker: "a://principal/gizzi", agentic: true, ...(agentic.bot_id ? { bot_id: agentic.bot_id } : {}), ...outputs },
              }),
            }).then(() => {}).catch(() => {}),
          // P3.1: render + attach a finished document to this run.
          createDeliverable: (input) =>
            api<{ url?: string; file?: string; error?: string; message?: string }>(
              `/cowork/runs/${grant.run_id}/deliverables`,
              {
                method: "POST",
                body: JSON.stringify(input),
              },
            )
              .then((res) => {
                log("info", "worker.deliverable_attached", { run_id: grant.run_id, file: res.file ?? res.url })
                return { ok: true, detail: `deliverable attached: ${res.url ?? res.file ?? "ok"}` }
              })
              .catch((e) => {
                log("error", "worker.deliverable_failed", { run_id: grant.run_id, error: (e as Error).message })
                return { ok: false, detail: (e as Error).message }
              }),
          log,
          system: agentic.system,
          maxSteps: agentic.max_steps,
          maxTokens: agentic.max_tokens,
        })
        continue
      }

      for (let i = 0; i < steps.length; i++) {
        const { code } = await runStep(steps[i], cwd, grant.job_id)
        stepResults.push({ step: i, code })
        if (code !== 0) {
          failed = true
          break
        }
        await api(`/runs/${grant.run_id}/checkpoints`, {
          method: "POST",
          body: JSON.stringify({
            step_index: i,
            cursor_state: { completed_steps: i + 1, by: "a://principal/gizzi" },
          }),
        }).catch(() => {})
      }
    } finally {
      if (heartbeat) clearInterval(heartbeat)
    }

    const outcome = await api<{ outcome: string; result?: { result_id?: string } }>(
      `/fabric/transport/jobs/${grant.job_id}/complete`,
      {
        method: "POST",
        body: JSON.stringify({
          lease_id: grant.lease_id,
          lease_generation: grant.lease_generation,
          success: !failed,
          summary: failed
            ? `gizzi: step failed (${stepResults.at(-1)?.step})`
            : `gizzi: ${steps.length} steps executed`,
          outputs: { worker: "a://principal/gizzi", steps: stepResults },
        }),
      },
    ).catch((e) => ({ outcome: "error", result: undefined, error: (e as Error).message }))
    log(failed ? "error" : "info", "worker.completed", {
      job_id: grant.job_id,
      outcome: outcome.outcome,
      steps: stepResults.length,
    })
  }
}
