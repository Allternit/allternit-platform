// Same name and description as the runtime builtin (runtime/tools/builtins/wait-agents.ts).
import WAIT_AGENTS_TXT from '@/runtime/tools/builtins/wait-agents.txt'
import { WAIT_AGENTS_TOOL_ID } from '@/runtime/primitives/wave-b-types'

export const WAIT_AGENTS_TOOL_NAME = WAIT_AGENTS_TOOL_ID

export const DESCRIPTION = 'Wait for your background agents to finish'

// In the terminal, agents are started with the Agent tool (run_in_background).
export const PROMPT: string = `${WAIT_AGENTS_TXT.trim()}

In this terminal, background agents are the ones you start with the Agent tool (run_in_background: true). Their ids are the agent ids that tool returns; a name you gave one works as "/root/<name>".`
