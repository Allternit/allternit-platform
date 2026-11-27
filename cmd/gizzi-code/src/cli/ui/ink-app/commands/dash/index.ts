import type { Command } from '../../commands'

// Merged into /sessions (SLASH-LIST MERGE): a hidden name that opens the sessions view.
const dash = {
  type: 'local-jsx',
  name: 'dash',
  description: 'Open the live sessions view (same as /sessions)',
  isHidden: true,
  immediate: true,
  load: () => import('../sessions/sessions.js'),
} satisfies Command
export default dash
