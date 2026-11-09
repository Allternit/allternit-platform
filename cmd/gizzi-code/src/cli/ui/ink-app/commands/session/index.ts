import { getIsRemoteMode } from '../../bootstrap/state'
import type { Command } from '../../commands'

const session = {
  type: 'local-jsx',
  name: 'session',
  description: 'Show remote session URL and QR code',
  isEnabled: () => getIsRemoteMode(),
  get isHidden() {
    return !getIsRemoteMode()
  },
  load: () => import('./session.js'),
} satisfies Command
export default session
