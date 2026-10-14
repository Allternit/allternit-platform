import { OpenMausBotProvider } from './provider.js';
export function createAaiRegistration(env: NodeJS.ProcessEnv) {
  return { provider: new OpenMausBotProvider({ baseUrl: env.SUBS_GATEWAY_OPENMAUSBOT_URL, token: env.SUBS_GATEWAY_OPENMAUSBOT_TOKEN }) };
}
