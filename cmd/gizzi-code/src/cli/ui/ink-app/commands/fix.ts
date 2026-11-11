import type { ContentBlockParam } from '@allternit/gizzi-sdk/providers/allternit/resources/messages.js'
import type { Command } from '../commands.js'
import { buildFixPrompt, getLastPetFailure } from '../pet/petOffers.js'

/** Accepts a pet offer: asks the model to fix the last failed command, with the matched line and output tail. */
const fix: Command = {
  type: 'prompt',
  name: 'fix',
  description: 'Fix the last failed command or error (your pet offers this after a failure)',
  progressMessage: 'looking at the failure',
  contentLength: 0,
  source: 'builtin',
  async getPromptForCommand(): Promise<ContentBlockParam[]> {
    return [{ type: 'text', text: buildFixPrompt(getLastPetFailure()) }]
  },
}

export default fix
