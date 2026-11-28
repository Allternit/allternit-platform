import type { Command } from '../../commands'

/** Hidden alias of /status (overhaul plan D4): opens the Stats tab. */
const stats = {
  type: 'local-jsx',
  name: 'stats',
  description: 'Open /status on the Stats tab',
  isHidden: true,
  load: () => import('../status/alias.js').then(m => ({ call: m.statsTabCall })),
} satisfies Command
export default stats
