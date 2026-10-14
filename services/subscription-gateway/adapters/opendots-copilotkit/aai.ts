import { OpenDotsCopilotkitProvider } from './provider.js';
export function createAaiRegistration(env:NodeJS.ProcessEnv){return {provider:new OpenDotsCopilotkitProvider({baseUrl:env.SUBS_GATEWAY_OPENDOTS_COPILOTKIT_URL,token:env.SUBS_GATEWAY_OPENDOTS_COPILOTKIT_TOKEN,realtimeOrigin:env.SUBS_GATEWAY_OPENDOTS_COPILOTKIT_REALTIME_ORIGIN})};}
