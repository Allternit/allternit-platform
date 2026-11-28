import type { Command } from '../../commands.js'

const keepGoing = {
  type: 'local',
  name: 'keep-going',
  description: 'Keep going on Allternit Cloud: continue a chat paused at your subscription\'s usage limit on Cloud credits',
  // Hidden alias: `/resume keep-going` is the visible form (overhaul plan D4).
  isHidden: true,
  supportsNonInteractive: false,
  load: () => import('./keep-going.js'),
} satisfies Command

export default keepGoing
