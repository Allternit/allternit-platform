import type { Command } from '../../commands'

const away = {
  type: 'local',
  name: 'away',
  description:
    'Show what happened while you were away: answered, waiting, finished and suggested',
  supportsNonInteractive: true,
  load: () => import('./away.js'),
} satisfies Command

export default away
