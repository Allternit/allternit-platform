import { OpenBotNightlyProvider } from './provider.js';
export function createAaiRegistration(env:NodeJS.ProcessEnv){return{provider:new OpenBotNightlyProvider({baseUrl:env.SUBS_GATEWAY_OPENBOT_NIGHTLY_URL,token:env.SUBS_GATEWAY_OPENBOT_NIGHTLY_TOKEN,appVersion:env.SUBS_GATEWAY_OPENBOT_NIGHTLY_APP_VERSION})};}
