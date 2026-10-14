import { CodyncProvider } from './provider.js';
export function createAaiRegistration(env: NodeJS.ProcessEnv) {
  return { provider: new CodyncProvider({ baseUrl: env.SUBS_GATEWAY_CODYNC_URL, token: env.SUBS_GATEWAY_CODYNC_TOKEN }) };
}
