import type { Command } from '../../commands.js'

/**
 * `/runtime` (Harness & Bots 2.0, plan D7): pick the runtime (how it works)
 * and the brain (how it thinks) for this session, or for the bot in a /bots
 * chat. `/model` stays the quick brain-only switch.
 */
export default {
  type: 'local-jsx',
  name: 'runtime',
  description: 'Choose the runtime (how it works) and brain (how it thinks)',
  argumentHint: '[runtime]',
  load: () => import('./runtime.js'),
} satisfies Command
