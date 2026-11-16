import { getIsNonInteractiveSession } from '../../bootstrap/state'
import type { Command } from '../../commands'

export const context: Command = {
  name: 'context',
  description: 'Visualize current context usage as a colored grid; `/context files` lists the files in context',
  argumentHint: '[files]',
  isEnabled: () => !getIsNonInteractiveSession(),
  type: 'local-jsx',
  load: () => import('./context.js'),
}

export const contextNonInteractive: Command = {
  type: 'local',
  name: 'context',
  supportsNonInteractive: true,
  description: 'Show current context usage; `/context files` lists the files in context',
  argumentHint: '[files]',
  get isHidden() {
    return !getIsNonInteractiveSession()
  },
  isEnabled() {
    return getIsNonInteractiveSession()
  },
  load: () => import('./context-noninteractive.js'),
}
