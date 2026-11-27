import type { Command } from '../../commands.js'

export default {
  type: 'local',
  name: 'knowledge',
  description: 'Review knowledge flags and proposals from the curator',
  argumentHint: '[list|show <id>|approve <id>|reject <id>|dismiss <id>]',
  supportsNonInteractive: false,
  load: () => import('./knowledge.js'),
} satisfies Command
