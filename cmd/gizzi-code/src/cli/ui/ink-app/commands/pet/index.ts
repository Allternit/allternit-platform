import type { Command } from '../../commands.js'

const pet = {
  type: 'local',
  name: 'pet',
  description: 'Hatch, pat, or mute your terminal pet',
  argumentHint: '[pat|mute|unmute]',
  supportsNonInteractive: false,
  load: () => import('./pet.js'),
} satisfies Command

export default pet
