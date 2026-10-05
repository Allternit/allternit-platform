import type { Command } from '../../commands'

const factory = {
  type: 'local-jsx',
  name: 'factory',
  description:
    'Open the Allternit Factory floor — bots of every binding, what needs you, the board, the live wall',
  immediate: true,
  load: () => import('./factory.js'),
} satisfies Command
export default factory
