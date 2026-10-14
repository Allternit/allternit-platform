import { MunderDifflinProvider } from './provider.js';
export function createAaiRegistration(env: NodeJS.ProcessEnv) { return { provider: new MunderDifflinProvider({ baseUrl: env.SUBS_GATEWAY_MUNDER_DIFFLIN_URL, endpointId: env.SUBS_GATEWAY_MUNDER_DIFFLIN_ENDPOINT_ID, secret: env.SUBS_GATEWAY_MUNDER_DIFFLIN_SECRET }) }; }
