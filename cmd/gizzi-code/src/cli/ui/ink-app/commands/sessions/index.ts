import type { Command } from '../../commands'

const sessions = {
  type: 'local-jsx',
  name: 'sessions',
  description: 'Live sessions: every gizzi session and Factory pane, working / blocked / idle (enter attach · s steer · n new · x stop)',
  immediate: true,
  load: () => import('./sessions.js'),
} satisfies Command
export default sessions
