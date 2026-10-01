// AAI registration: Kimi turns run as the gateway's own chat tasks (VendorContext.gatewayTasks).
import { create } from "./index.js";
import type { GatewayTasks } from "../_shared/subscription-agent.js";

export function createAaiRegistration(_env: NodeJS.ProcessEnv, ctx: { gatewayTasks?: GatewayTasks } = {}) {
  return { provider: create({ tasks: ctx.gatewayTasks }) };
}
