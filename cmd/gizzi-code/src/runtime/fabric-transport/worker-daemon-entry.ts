/**
 * Daemon entry point for the Gizzi fabric-transport worker (P-T3).
 *
 *   ALLTERNIT_GIZZI_TOKEN=atok_… \
 *     bun src/runtime/fabric-transport/worker-daemon-entry.ts
 *
 * Differences from the plain `worker-entry.ts` loop (claim protocol
 * unchanged):
 *   - structured JSON logs, one event per line (`{ts, level, event, ...}`);
 *   - exponential backoff + jitter on transport/claim errors
 *     (1s → 2s → … → 30s cap, reset on the next successful claim);
 *   - graceful shutdown on SIGTERM/SIGINT: stops claiming, finishes the
 *     in-flight job, then exits. A lease abandoned this way is released by
 *     the standard lease-expiry path (heartbeats stop; the control-plane
 *     sweeper requeues the job within ~`ALLTERNIT_GIZZI_LEASE_SECS`) — the
 *     worker never completes another principal's work and never writes a
 *     terminal result for a healthy job.
 */

import { runFabricWorker } from "./worker"

type Level = "info" | "warn" | "error"

function makeLogger() {
  return (level: Level, event: string, fields?: Record<string, unknown>) => {
    const line = JSON.stringify({ ts: new Date().toISOString(), level, event, ...(fields ?? {}) })
    if (level === "error") console.error(line)
    else console.log(line)
  }
}

let stopping = false

const log = makeLogger()

function installSignalHandlers(): void {
  const onSignal = (signal: string) => {
    if (stopping) return
    stopping = true
    log("info", "worker.shutdown_requested", { signal })
    // The run loop notices `shouldStop()` between claims (an in-flight job
    // is finished first, an in-flight claim long-poll returns within its
    // wait window) and then exits with code 0.
  }
  process.on("SIGTERM", () => onSignal("SIGTERM"))
  process.on("SIGINT", () => onSignal("SIGINT"))
}

installSignalHandlers()
log("info", "worker.daemon_start", { compute_mode: process.env.GIZZI_COMPUTE_MODE ?? "local" })

runFabricWorker({
  daemon: {
    log,
    shouldStop: () => stopping,
  },
})
  .then(() => {
    log("info", "worker.daemon_stopped", {})
    process.exit(0)
  })
  .catch((err) => {
    log("error", "worker.fatal", { error: (err as Error).message })
    process.exit(1)
  })
