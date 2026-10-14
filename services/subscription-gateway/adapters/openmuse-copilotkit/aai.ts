import { OpenMuseCopilotkitProvider } from './provider.js';
export function createAaiRegistration(env:NodeJS.ProcessEnv){return {provider:new OpenMuseCopilotkitProvider({baseUrl:env.SUBS_GATEWAY_OPENMUSE_COPILOTKIT_URL,token:env.SUBS_GATEWAY_OPENMUSE_COPILOTKIT_TOKEN})};}
