import { VellumAssistantProvider } from './provider.js';
export function createAaiRegistration(env: NodeJS.ProcessEnv) { return { provider: new VellumAssistantProvider({ baseUrl: env.SUBS_GATEWAY_VELLUM_ASSISTANT_URL, token: env.SUBS_GATEWAY_VELLUM_ASSISTANT_TOKEN }) }; }
