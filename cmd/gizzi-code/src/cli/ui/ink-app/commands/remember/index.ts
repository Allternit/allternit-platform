import type { Command } from '../../commands'

const remember = {
  type: 'local',
  name: 'remember',
  // Hidden alias of /memory (overhaul plan D4): still runs when typed; /memory add is the listed way.
  isHidden: true,
  description: 'Save a note to memory immediately, without waiting for a summary',
  argumentHint: '[--user|--project|--local] <note>',
  supportsNonInteractive: true,
  load: () => import('./remember.js'),
} satisfies Command

export default remember
