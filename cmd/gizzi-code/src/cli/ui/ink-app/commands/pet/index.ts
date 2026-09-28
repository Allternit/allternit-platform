import type { Command } from '../../commands.js'

const pet = {
  type: 'local-jsx',
  name: 'pet',
  description: 'Open your pet: quick chat with your Allternit bot, incognito asks, switch bots',
  argumentHint: '[pat|mute|unmute]',
  immediate: true,
  load: () => import('./pet.js'),
} satisfies Command

export default pet
