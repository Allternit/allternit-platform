// AAI registration (loaded at boot by registerVendorAdapters). Non-secret config from env; the optional gateway
// bearer token is read here only to authenticate to the user's own local OpenClaw and is never logged or echoed.
import { openclaw } from "./index.js";

export function createAaiRegistration(env: NodeJS.ProcessEnv) {
  const maxParallel = Number(env.SUBS_GATEWAY_OPENCLAW_MAX_PARALLEL);
  return {
    provider: openclaw.create({
      baseUrl: env.SUBS_GATEWAY_OPENCLAW_URL || undefined,
      token: env.SUBS_GATEWAY_OPENCLAW_TOKEN || undefined,
      defaultAgent: env.SUBS_GATEWAY_OPENCLAW_AGENT || undefined,
      maxParallel: Number.isInteger(maxParallel) && maxParallel > 0 ? maxParallel : undefined,
    }),
  };
}
