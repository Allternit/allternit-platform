import type { Command } from '../../commands'

const memorySearch: Command = {
  type: 'local-jsx',
  name: 'memory-search',
  description: 'Search your Memory Drive',
  argumentHint: '<words>',
  load: () => import('./memory-search.js'),
}
export default memorySearch
