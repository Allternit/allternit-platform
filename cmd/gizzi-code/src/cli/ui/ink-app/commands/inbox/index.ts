import type { Command } from '../../commands.js'

/**
 * /inbox — what is waiting on you. You approve or deny each item with a key
 * press; the model has no way to do either (AllternitOS CONTRACT 7.2). The
 * calls are the shared client in runtime/services/api/inbox.ts, as for
 * `gizzi inbox`.
 */
const inbox = {
  type: 'local-jsx',
  name: 'inbox',
  description: 'See what needs you and approve or deny it',
  load: () => import('./inbox.js'),
} satisfies Command

export default inbox
