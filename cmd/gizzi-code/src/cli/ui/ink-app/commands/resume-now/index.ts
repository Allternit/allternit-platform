import type { Command } from '../../commands.js'

const resumeNow = {
  type: 'local',
  name: 'resume-now',
  description: 'Continue a chat paused before a usage limit now, on the suggested model if there is one',
  // Hidden alias: `/resume now` is the visible form (overhaul plan D4).
  isHidden: true,
  supportsNonInteractive: false,
  load: () => import('./resume-now.js'),
} satisfies Command

export default resumeNow
