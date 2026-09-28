import type { ContentBlockParam } from '@allternit/gizzi-sdk/providers/allternit/resources/messages.js'
import type { Command } from '../../commands.js'
import { AGENT_TOOL_NAME } from '../../tools/AgentTool/constants.js'
import { isForkSubagentEnabled } from '../../tools/AgentTool/forkSubagent.js'

/**
 * `/fork <directive>`: start a background fork that inherits this whole
 * conversation and works on the directive, reporting back with a task
 * notification. It goes through the Agent tool's fork path (Agent with no
 * subagent_type), so the fork shares the parent's prompt cache and shows up
 * in the tasks list like any other background agent. `/branch` copies the
 * conversation into a new session instead.
 */
const fork = {
  type: 'prompt',
  name: 'fork',
  description:
    'Start a background fork of this conversation to work on a directive',
  argumentHint: '<directive>',
  progressMessage: 'Starting a fork',
  contentLength: 0,
  source: 'builtin',
  isEnabled: () => isForkSubagentEnabled(),
  async getPromptForCommand(args): Promise<ContentBlockParam[]> {
    const directive = (args ?? '').trim()
    if (!directive) {
      return [
        {
          type: 'text',
          text: 'The user ran /fork without a directive. Tell them in one line: usage is /fork <directive>, which starts a background fork of this conversation that works on the directive and reports back. Do not call any tools.',
        },
      ]
    }
    return [
      {
        type: 'text',
        text: `The user ran /fork. Call the ${AGENT_TOOL_NAME} tool once, now, WITHOUT subagent_type (that starts a fork that inherits this conversation), with a 3-5 word description and this exact prompt:

<directive>
${directive}
</directive>

Pass the directive verbatim as the prompt. Do not do the work yourself and do not call other tools first. After the call, say in one line that the fork is running.`,
      },
    ]
  },
} satisfies Command

export default fork
