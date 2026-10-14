import { DsBotProvider } from './provider.js';
export function createAaiRegistration(env:NodeJS.ProcessEnv){return{provider:new DsBotProvider({baseUrl:env.SUBS_GATEWAY_DS_BOT_URL,token:env.SUBS_GATEWAY_DS_BOT_TOKEN})};}
