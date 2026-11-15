import type { Command } from '../../commands.js'

/**
 * /mission <goal> — plan a long-running goal as milestones, approve it, and let
 * the background runner work through them. Logic is cli/mission/actions.ts,
 * shared with `gizzi mission`.
 */
const mission = {
  type: 'local-jsx',
  name: 'mission',
  aliases: ['missions'],
  description: 'Plan a goal as milestones, approve the plan, and run it in the background',
  argumentHint: '<goal> [--brain <brain>] [--on <computer>] [--max-workers N] [--max-hours N]',
  load: () => import('./mission.js'),
} satisfies Command

export default mission
