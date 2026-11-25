import type { Command } from '../../commands'

const share = {
  type: 'local',
  name: 'share',
  description:
    'Share this conversation as a read-only link (text and tool names only). /share off turns the link off. /share links lists your share links (pause, resume, stop)',
  supportsNonInteractive: false,
  argumentHint: '[off | links | pause <id> | resume <id> | stop <id>]',
  load: () => import('./share.js'),
} satisfies Command
export default share
