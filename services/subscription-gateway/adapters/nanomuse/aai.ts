import { NanoMuseProvider } from './provider.js';
export function createAaiRegistration(env: NodeJS.ProcessEnv) { return { provider: new NanoMuseProvider({ baseUrl: env.SUBS_GATEWAY_NANOMUSE_URL, token: env.SUBS_GATEWAY_NANOMUSE_TOKEN }) }; }
