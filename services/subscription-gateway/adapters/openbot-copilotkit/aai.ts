import { OpenBotCopilotkitProvider } from './provider.js';
export function createAaiRegistration(env: NodeJS.ProcessEnv) {
  return { provider: new OpenBotCopilotkitProvider({ realtimeOrigin: env.SUBS_GATEWAY_OPENBOT_COPILOTKIT_REALTIME_ORIGIN }) };
}
