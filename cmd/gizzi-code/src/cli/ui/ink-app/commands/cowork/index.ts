import type { Command } from '../../commands.js'

const cowork = {
  type: 'local-jsx',
  name: 'cowork',
  aliases: ['cw'], // 'board' is the Factory board (/board)
  description: 'Interactive workspace tasks Kanban board and schedule management',
  load: () => import('./cowork.js'),
} satisfies Command

export default cowork
