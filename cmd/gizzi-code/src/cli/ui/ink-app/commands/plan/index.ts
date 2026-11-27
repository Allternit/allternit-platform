import type { Command } from '../../commands'

const plan = {
  type: 'local-jsx',
  name: 'plan',
  description: 'Enable plan mode or view the current session plan',
  argumentHint: '[open|view|<description>]',
  load: () => import('./plan.js'),
} satisfies Command
export default plan
