/**
 * /cost is a hidden alias of /status (overhaul plan D4): in the terminal UI it
 * opens the Cost tab. The text form stays for print mode, --remote and Remote
 * Control, which cannot show a tabbed view.
 */
import { getIsNonInteractiveSession } from '../../bootstrap/state'
import type { Command } from '../../commands'

const cost = {
  type: 'local',
  name: 'cost',
  description: 'Show the total cost and duration of the current session',
  isHidden: true,
  supportsNonInteractive: true,
  load: () => import('./cost.js'),
} satisfies Command
export default cost

export const costTab = {
  type: 'local-jsx',
  name: 'cost',
  description: 'Open /status on the Cost tab',
  isHidden: true,
  isEnabled: () => !getIsNonInteractiveSession(),
  load: () => import('../status/alias.js').then(m => ({ call: m.costTabCall })),
} satisfies Command
