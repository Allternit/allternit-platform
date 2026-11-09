import type { Command } from '../../commands'
import { isEnvTruthy } from '../../utils/envUtils'

export default {
  type: 'local-jsx',
  name: 'logout',
  description: 'Sign out of Gizzi Code',
  isEnabled: () => !isEnvTruthy(process.env.DISABLE_LOGOUT_COMMAND),
  load: () => import('./logout.js'),
} satisfies Command
