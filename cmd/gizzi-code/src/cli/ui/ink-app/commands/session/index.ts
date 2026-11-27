import { getIsRemoteMode } from '../../bootstrap/state'
import type { Command } from '../../commands'

const session = {
  type: 'local-jsx',
  name: 'session',
  description: 'Show remote session URL and QR code',
  isEnabled: () => getIsRemoteMode(),
  // Hidden alias of /remote (overhaul plan D4): still runs when typed; /remote session is the listed way.
  isHidden: true,
  load: () => import('./session.js'),
} satisfies Command
export default session
