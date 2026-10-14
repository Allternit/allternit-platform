import { InvisibleDotsProvider } from './provider.js';
export function createAaiRegistration(env:NodeJS.ProcessEnv){return {provider:new InvisibleDotsProvider({baseUrl:env.SUBS_GATEWAY_INVISIBLE_DOTS_URL,token:env.SUBS_GATEWAY_INVISIBLE_DOTS_TOKEN})};}
