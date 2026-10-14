import { SidematesProvider } from './provider.js';
export function createAaiRegistration(env: NodeJS.ProcessEnv) { return { provider: new SidematesProvider({ baseUrl: env.SUBS_GATEWAY_SIDEMATES_URL, token: env.SUBS_GATEWAY_SIDEMATES_TOKEN }) }; }
