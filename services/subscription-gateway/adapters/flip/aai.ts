// Messages-lane AAI registration: texts the Flip assistant through the user's own macOS Messages.
import { createMessagesProvider } from '../_shared/messages-local.js';

export function createAaiRegistration(env: NodeJS.ProcessEnv) {
  return { provider: createMessagesProvider({ adapterId: 'flip', displayName: 'Flip', defaultHandle: '+16282647696' }, { env }) };
}
