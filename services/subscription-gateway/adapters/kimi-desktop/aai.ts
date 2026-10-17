// AAI registration (loaded at boot by registerVendorAdapters). Lazy: nothing touches Kimi until the user has consented
// (SUBS_GATEWAY_KIMI_DESKTOP_AX_CONSENT=1) AND the ax-bridge binary is named (SUBS_GATEWAY_AX_BRIDGE_BIN). Until then every
// call answers LANE_BLOCKED (no consent) or VENDOR_UNAVAILABLE (no bridge), and the gateway stays fully usable.
import { unavailableAxDriver } from "../_shared/ax/index.js";
import { createKimiDesktopProvider, PACING } from "./index.js";

export function createAaiRegistration(env: NodeJS.ProcessEnv) {
  const bin = env.SUBS_GATEWAY_AX_BRIDGE_BIN;
  return {
    provider: createKimiDesktopProvider({
      ...(bin ? { axBinPath: bin } : { ax: unavailableAxDriver("The Allternit accessibility helper (ax-bridge) is not installed on this computer.") }),
      consented: env.SUBS_GATEWAY_KIMI_DESKTOP_AX_CONSENT === "1",
    }),
    pacing: { minGapMs: PACING.min_task_gap_s * 1000, maxPerHour: PACING.max_tasks_per_hour },
  };
}
