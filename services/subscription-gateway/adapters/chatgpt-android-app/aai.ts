// AAI registration (loaded at boot by registerVendorAdapters). Reaches the phone only through
// Allternit Desktop's phone gateway on this computer; nothing happens until a bot opens a chat.
import { DesktopPhoneGateway } from "../../src/phone/gateway.js";
import { create, PACING } from "./index.js";

export function createAaiRegistration(env: NodeJS.ProcessEnv) {
  return {
    provider: create({ gateway: new DesktopPhoneGateway({ file: env.SUBS_GATEWAY_PHONE_GATEWAY_FILE }) }),
    pacing: { minGapMs: PACING.min_task_gap_s * 1000, maxPerHour: PACING.max_tasks_per_hour },
  };
}
