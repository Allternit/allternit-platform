import type { Command } from '../../commands'

/**
 * Hidden alias of /status (overhaul plan D4): opens the Status tab, which shows
 * the same model, directory, session id and context (c copies the session id).
 */
const sessionInfo = {
  type: 'local-jsx',
  name: 'session-info',
  description: 'Open /status on the Status tab',
  isHidden: true,
  immediate: true,
  load: () => import('../status/alias.js').then(m => ({ call: m.statusTabCall })),
} satisfies Command

export default sessionInfo
