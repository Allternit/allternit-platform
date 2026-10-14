import { FrockBotProvider } from './provider.js';

export function createAaiRegistration(env:NodeJS.ProcessEnv) {
 return {provider:new FrockBotProvider({baseUrl:env.SUBS_GATEWAY_FROCKBOT_URL,token:env.SUBS_GATEWAY_FROCKBOT_DEBUG_TOKEN,userId:env.SUBS_GATEWAY_FROCKBOT_ADMIN_USER_ID,enabled:env.SUBS_GATEWAY_FROCKBOT_DEBUG_ENABLED==='1'})};
}
