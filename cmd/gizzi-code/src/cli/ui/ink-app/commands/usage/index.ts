import type { Command } from '../../commands'

export default {
  type: 'local',
  name: 'usage',
  aliases: ['tokens'],
  description: 'Show your Allternit plan, credits and CLI-brain limits, plus this session\'s tokens and cost',
  supportsNonInteractive: true,
  load: () => import('./usage.js'),
} satisfies Command
