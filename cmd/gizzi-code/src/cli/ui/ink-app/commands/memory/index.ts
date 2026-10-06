import type { Command } from '../../commands'

const memory: Command = {
  type: 'local-jsx',
  name: 'memory',
  aliases: ['mem'],
  description: 'Show your Memory Drive and edit memory files',
  argumentHint: '[view <path>|log|sync]',
  load: () => import('./memory.js'),
}
export default memory
