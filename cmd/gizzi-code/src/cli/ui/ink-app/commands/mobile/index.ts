import type { Command } from '../../commands'

const mobile = {
  type: 'local-jsx',
  name: 'mobile',
  aliases: ['ios', 'android'],
  // Hidden alias of /remote (overhaul plan D4): still runs when typed; /remote mobile is the listed way.
  isHidden: true,
  description: 'Show QR code to download the Allternit mobile app',
  load: () => import('./mobile.js'),
} satisfies Command
export default mobile
