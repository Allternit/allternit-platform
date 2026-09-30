// AAI registration for the subscription gateway's AaiHost (loaded at boot by
// registerVendorAdapters). The CDP driver connects lazily: nothing touches
// Claude until the user has consented to start it with a debugging port.
import { claudeDesktop, CLAUDE_DESKTOP_CDP_PORT, PACING } from "./index.js";

export function createAaiRegistration(env: NodeJS.ProcessEnv) {
  const port = Number(env.SUBS_GATEWAY_CLAUDE_DESKTOP_CDP_PORT ?? CLAUDE_DESKTOP_CDP_PORT);
  // SUBS_GATEWAY_CLAUDE_DESKTOP_TRANSPORT=ax drives Claude through macOS Accessibility (no debug port). It stays LANE_BLOCKED
  // until SUBS_GATEWAY_CLAUDE_AX_CONSENT=1 and the ax-bridge binary is named in SUBS_GATEWAY_AX_BRIDGE_BIN.
  const ax = env.SUBS_GATEWAY_CLAUDE_DESKTOP_TRANSPORT === "ax" && !!env.SUBS_GATEWAY_AX_BRIDGE_BIN;
  return {
    provider: ax
      ? claudeDesktop.create({ transport: "ax", axBinPath: env.SUBS_GATEWAY_AX_BRIDGE_BIN, axConsented: env.SUBS_GATEWAY_CLAUDE_AX_CONSENT === "1" })
      : claudeDesktop.create({ cdpPort: port }),
    pacing: { minGapMs: PACING.min_task_gap_s * 1000, maxPerHour: PACING.max_tasks_per_hour },
  };
}
