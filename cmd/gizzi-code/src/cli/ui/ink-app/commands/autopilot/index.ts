import type { Command } from '../../commands'

const autopilot = {
  type: 'local',
  name: 'autopilot',
  description:
    'Show or switch Autopilot, your main bot answering for you: /autopilot [status|on|off]',
  argumentHint: '[status|on|off]',
  supportsNonInteractive: true,
  load: () => import('./autopilot.js'),
} satisfies Command

export default autopilot
