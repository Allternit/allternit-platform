// AAI registration for the subscription gateway's AaiHost (loaded at boot by registerVendorAdapters). The browser
// driver is lazy: nothing opens Chrome or touches chatgpt.com until the user has consented
// (SUBS_GATEWAY_DOTS_CONSENT=1) AND named the account profile (SUBS_GATEWAY_DOTS_PROFILE_DIR). Without them every call
// answers LANE_BLOCKED / VENDOR_UNAVAILABLE, and the gateway stays fully usable.
import { chatgptDots, PACING } from "./index.js";

export function createAaiRegistration(env: NodeJS.ProcessEnv) {
  return {
    provider: chatgptDots.create({
      // SUBS_GATEWAY_DOTS_TRANSPORT=chatgpt-app: native ChatGPT.app via macOS Accessibility (needs the bridge bin + explicit consent).
      transport: env.SUBS_GATEWAY_DOTS_TRANSPORT === "chatgpt-app" && env.SUBS_GATEWAY_AX_BRIDGE_BIN ? "chatgpt-app" : "browser",
      axBinPath: env.SUBS_GATEWAY_AX_BRIDGE_BIN || undefined,
      axConsented: env.SUBS_GATEWAY_CHATGPT_APP_AX_CONSENT === "1",
      profileDir: env.SUBS_GATEWAY_DOTS_PROFILE_DIR || undefined,
      userConsented: env.SUBS_GATEWAY_DOTS_CONSENT === "1",
      dotRef: env.SUBS_GATEWAY_DOTS_DEFAULT_DOT || undefined,
    }),
    pacing: { minGapMs: PACING.min_task_gap_s * 1000, maxPerHour: PACING.max_tasks_per_hour },
  };
}
