// Messages-lane AAI registration: texts the Iris assistant through the user's own macOS Messages.
import { createMessagesProvider } from '../_shared/messages-local.js';

export function createAaiRegistration(env: NodeJS.ProcessEnv) {
  return { provider: createMessagesProvider({ adapterId: 'iris', displayName: 'Iris' }, { env }) };
}
