// AAI registration (loaded at boot by registerVendorAdapters). Turns run as the gateway's own chat tasks, so the
// gateway hands this adapter its in-process task client (VendorContext.gatewayTasks). Without it, every call answers
// VENDOR_UNAVAILABLE and the rest of the gateway is unaffected.
import { claudeSubscription } from "./index.js";
import type { GatewayTasks } from "./provider.js";

export function createAaiRegistration(_env: NodeJS.ProcessEnv, ctx: { gatewayTasks?: GatewayTasks } = {}) {
  return { provider: claudeSubscription.create({ tasks: ctx.gatewayTasks }) };
}
