import type { Command } from '../../commands'

const resume: Command = {
  type: 'local-jsx',
  name: 'resume',
  description: 'Resume a previous conversation',
  aliases: ['continue'],
  argumentHint: '[conversation id or search term | now | keep-going]',
  load: () => import('./resume.js'),
}
export default resume
