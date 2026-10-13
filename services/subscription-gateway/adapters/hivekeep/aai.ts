import { HivekeepProvider } from './provider.js';
export function createAaiRegistration(env: NodeJS.ProcessEnv) { return { provider: new HivekeepProvider({ baseUrl: env.SUBS_GATEWAY_HIVEKEEP_URL, token: env.SUBS_GATEWAY_HIVEKEEP_TOKEN }) }; }
