import type { Command } from '../../commands'

const board = {
  type: 'local-jsx',
  name: 'board',
  description: 'Live Factory board: every running bot and swarm worker, its state, age and last line (enter attach · s steer · x stop · l land)',
  immediate: true,
  load: () => import('./board.js'),
} satisfies Command
export default board
