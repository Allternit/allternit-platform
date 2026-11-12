import type { Command } from '../../commands'

const share = {
  type: 'local',
  name: 'share',
  description:
    'Share this conversation as a read-only link (text and tool names only). /share off turns the link off',
  supportsNonInteractive: false,
  argumentHint: '[off]',
  load: () => import('./share.js'),
} satisfies Command
export default share
