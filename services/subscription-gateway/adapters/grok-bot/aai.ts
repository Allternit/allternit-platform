// AAI registration for the subscription gateway's AaiHost (loaded at boot by
// registerVendorAdapters). The CDP driver connects lazily: nothing touches
// Grok Bot until the user has consented to start it with a debugging port.
import { grokBot, PACING } from "./index.js";

export function createAaiRegistration(env: NodeJS.ProcessEnv) {
  const port = Number(env.SUBS_GATEWAY_GROK_BOT_CDP_PORT ?? 9222);
  return {
    provider: grokBot.create({ cdpPort: port }),
    pacing: { minGapMs: PACING.min_task_gap_s * 1000, maxPerHour: PACING.max_tasks_per_hour },
  };
}
