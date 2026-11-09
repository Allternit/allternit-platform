import type { Command } from '../../commands'
import { liveSessionEnabled } from '../../services/liveSession'

// Allternit Desktop runs on macOS, Windows and Linux; the session opens through allternit://.
const desktop = {
  type: 'local',
  name: 'desktop',
  aliases: ['app'],
  description: 'Open this live session in Allternit Desktop',
  isEnabled: liveSessionEnabled,
  supportsNonInteractive: false,
  load: () => import('./desktop.js'),
} satisfies Command

export default desktop
