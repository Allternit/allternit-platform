import type { Command } from '../../commands'
import { liveSessionEnabled } from '../../services/liveSession'

const remote = {
  type: 'local-jsx',
  name: 'remote',
  aliases: ['remote-control', 'continue-on', 'phone'],
  description: 'Continue this session live on your phone, the web app or Desktop (QR code + links)',
  isEnabled: liveSessionEnabled,
  load: () => import('./remote.js'),
} satisfies Command

export default remote
