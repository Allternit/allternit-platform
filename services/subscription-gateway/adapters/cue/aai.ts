// Messages-lane AAI registration: texts the Cue assistant through the user's own macOS Messages.
import { createMessagesProvider } from '../_shared/messages-local.js';

export function createAaiRegistration(env: NodeJS.ProcessEnv) {
  return { provider: createMessagesProvider({ adapterId: 'cue', displayName: 'Cue' }, { env }) };
}
