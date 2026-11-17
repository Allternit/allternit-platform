import type { Command } from '../../commands.js'

const answer = {
  type: 'local',
  name: 'answer',
  description: 'Answer a question the agent asked for later (ask_user_later); with no id, list the waiting ones',
  argumentHint: '[id] [choice or text]',
  supportsNonInteractive: false,
  load: () => import('./answer.js'),
} satisfies Command

export default answer
