import type { Command } from '../../commands'

const status = {
  type: 'local-jsx',
  name: 'status',
  aliases: ['info'],
  description:
    'Status, usage, cost and stats in one place (Tab or ←/→ switches tabs)',
  argumentHint: '[usage|cost|stats|--json|--inline]',
  immediate: true,
  load: () => import('./status.js'),
} satisfies Command
export default status
