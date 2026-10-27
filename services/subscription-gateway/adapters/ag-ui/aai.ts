import { AgUiProvider } from './provider.js';
export function createAaiRegistration(env:NodeJS.ProcessEnv){return {provider:new AgUiProvider({credential:env.SUBS_GATEWAY_AG_UI_CREDENTIAL})};}
