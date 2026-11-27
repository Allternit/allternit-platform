import type { Command } from '../../commands.js'

export default {
  type: 'local-jsx',
  name: 'knowledge',
  description: 'Review knowledge flags and proposals, and turn on approved skills',
  argumentHint: '[list|show <id>|approve <id>|reject <id>|dismiss <id>|activate <id>]',
  load: () => import('./knowledge.js'),
} satisfies Command
