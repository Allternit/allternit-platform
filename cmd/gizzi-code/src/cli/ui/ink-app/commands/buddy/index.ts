import type { Command } from '../../commands.js'

const buddy = {
  type: 'local',
  name: 'buddy',
  description: 'Hatch, pet, or mute your terminal companion',
  argumentHint: '[pet|mute|unmute]',
  supportsNonInteractive: false,
  load: () => import('./buddy.js'),
} satisfies Command

export default buddy
