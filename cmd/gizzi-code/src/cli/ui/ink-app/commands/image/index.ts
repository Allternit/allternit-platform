import type { Command } from '../../commands.js'

const image = {
  type: 'local-jsx',
  name: 'image',
  description: 'Make a picture from your brief',
  argumentHint: '<what to draw>',
  load: () => import('./image.js'),
} satisfies Command

export default image
