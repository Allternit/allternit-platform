import type { Command } from '../../commands'

const memorySearch: Command = {
  type: 'local-jsx',
  name: 'memory-search',
  // Hidden alias of /memory (overhaul plan D4): still runs when typed; /memory search is the listed way.
  isHidden: true,
  description: 'Search your Memory Drive',
  argumentHint: '<words>',
  load: () => import('./memory-search.js'),
}
export default memorySearch
