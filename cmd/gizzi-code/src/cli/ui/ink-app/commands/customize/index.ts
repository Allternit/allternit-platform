import type { Command } from '../../commands'

const customize = {
  type: 'local-jsx',
  name: 'customize',
  aliases: ['catalog'],
  description: 'Browse the Customize catalog (A://TERNIT OFFICIAL skills, MCP servers, commands) and add items',
  immediate: true,
  load: () => import('./customize.js'),
} satisfies Command

export default customize
