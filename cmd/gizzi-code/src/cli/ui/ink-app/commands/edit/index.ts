import type { Command } from '../../commands'

const edit = {
  type: 'local',
  name: 'edit',
  aliases: ['edit-last'],
  description: 'Rewind to your last message and put it back in the prompt to edit and resend',
  supportsNonInteractive: false,
  load: () => import('./edit.js'),
} satisfies Command
export default edit
