/**
 * /usage is a hidden alias of /status (overhaul plan D4): in the terminal UI
 * it opens the Usage tab. The text form stays for print mode and --remote.
 */
import { getIsNonInteractiveSession } from '../../bootstrap/state'
import type { Command } from '../../commands'

export default {
  type: 'local',
  name: 'usage',
  aliases: ['tokens'],
  description: 'Show your Allternit plan, credits and CLI-brain limits, plus this session\'s tokens and cost',
  isHidden: true,
  supportsNonInteractive: true,
  load: () => import('./usage.js'),
} satisfies Command

export const usageTab = {
  type: 'local-jsx',
  name: 'usage',
  aliases: ['tokens'],
  description: 'Open /status on the Usage tab',
  isHidden: true,
  isEnabled: () => !getIsNonInteractiveSession(),
  load: () => import('../status/alias.js').then(m => ({ call: m.usageTabCall })),
} satisfies Command
