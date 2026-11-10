import type { Command } from '../../commands.js'

/**
 * /swarm — real Factory swarms (was a simulated visualizer). Owned by the
 * factory lane; the logic is cli/factory/swarm.ts, shared with `gizzi swarm`.
 */
const swarm = {
  type: 'local-jsx',
  name: 'swarm',
  aliases: ['swarms'],
  description: 'Hand a task to N CLI agents in their own worktrees and watch them on the board',
  argumentHint: '<task> [-n 2] [--brain <provider/model>]',
  load: () => import('./swarm.js'),
} satisfies Command

export default swarm
