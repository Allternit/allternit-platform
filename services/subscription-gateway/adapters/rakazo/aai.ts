import { RakazoProvider } from './provider.js';
export function createAaiRegistration(env:NodeJS.ProcessEnv){return {provider:new RakazoProvider({baseUrl:env.SUBS_GATEWAY_RAKAZO_URL,spaceId:env.SUBS_GATEWAY_RAKAZO_SPACE_ID})};}
