import { HappyProvider } from './provider.js';
export function createAaiRegistration(env: NodeJS.ProcessEnv) {
  return { provider: new HappyProvider({ serverUrl: env.SUBS_GATEWAY_HAPPY_URL }) };
}
