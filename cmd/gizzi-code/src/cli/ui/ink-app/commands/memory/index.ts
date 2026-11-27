import type { Command } from '../../commands'

const memory: Command = {
  type: 'local-jsx',
  name: 'memory',
  aliases: ['mem'],
  description: 'Show your Memory Drive and edit memory files',
  argumentHint: '[search <words>|add <note>|view <path>|log|sync|sleep [on|off]]',
  load: () => import('./memory.js'),
}
export default memory
