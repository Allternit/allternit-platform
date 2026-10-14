import { OpenDotsAnilProvider } from './provider.js';
export function createAaiRegistration(env: NodeJS.ProcessEnv) { return { provider: new OpenDotsAnilProvider({ baseUrl: env.SUBS_GATEWAY_OPENDOTS_ANIL_URL, userId: env.SUBS_GATEWAY_OPENDOTS_ANIL_USER_ID }) }; }
