import { GawkbotProvider } from './provider.js';
export function createAaiRegistration(env: NodeJS.ProcessEnv) { return { provider: new GawkbotProvider({ baseUrl: env.SUBS_GATEWAY_GAWKBOT_URL, token: env.SUBS_GATEWAY_GAWKBOT_TOKEN }) }; }
