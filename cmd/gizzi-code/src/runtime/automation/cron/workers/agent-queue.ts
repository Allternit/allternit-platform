/**
 * Agent Task Queue Worker
 *
 * Was: claim a cowork queue item, simulate the work and mark it complete.
 * Cowork tasks are Factory nodes now (stream F9), so this refuses instead of
 * reporting work that never ran. Kept registered so existing cron configs
 * fail loudly rather than vanish.
 *
 * Registered as a `function` type cron job in CronServiceEnhanced.
 *
 * Usage (cron job config):
 *   {
 *     type: "function",
 *     config: {
 *       module: "@/runtime/automation/cron/workers/agent-queue",
 *       function: "runAgentQueueWorker",
 *       args: [{ agentId: "gizzi-agent-1", agentRole: "cowork" }]
 *     }
 *   }
 */

import { createLogger } from "../utils/logger";

const log = createLogger("agent-queue-worker");

// TODO: use legacyEnv("ALLTERNIT_API_URL", "Allternit_API_URL") from
// @/shared/constants/cloudUrls when this worker next changes — left as-is to
// avoid behavior drift in this pass.
const API_BASE = (process.env.ALLTERNIT_API_URL ?? process.env.Allternit_API_URL) || "http://localhost:3001";

interface QueueWorkerConfig {
  agentId: string;
  agentRole?: string;
}

/**
 * Main entry point for the agent queue worker cron job.
 */
export async function runAgentQueueWorker(config: QueueWorkerConfig): Promise<void> {
  // Cowork tasks are Factory nodes now, and a claim moves a real task to
  // in-progress (then in-review on complete). This worker has no agent
  // runtime to do the work, so claiming would report work that never
  // happened. It refuses instead of simulating. Run real agents on tasks with
  // `gizzi workflows drive` (Factory nodes) or a bot's Work app.
  log.error("agent-queue-worker can't run tasks: no agent runtime is wired to it, so nothing was claimed", {
    agentId: config.agentId,
    action: "Use `gizzi workflows drive` to run agents on Factory nodes.",
  });
  // Throw so the cron run is recorded as failed, not as a quiet success.
  throw new Error(
    "agent-queue-worker can't run tasks: no agent runtime is wired to it, so nothing was claimed. Use `gizzi workflows drive` to run agents on Factory nodes.",
  );
}

/**
 * Health check for the agent queue worker.
 */
export async function healthCheck(): Promise<boolean> {
  try {
    const response = await fetch(`${API_BASE}/api/v1/tasks`, { method: "HEAD" });
    return response.ok || response.status === 405; // 405 Method Not Allowed is fine — API is up
  } catch {
    return false;
  }
}
