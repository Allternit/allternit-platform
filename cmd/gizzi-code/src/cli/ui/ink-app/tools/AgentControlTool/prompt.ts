// Same tool id as the runtime builtin (runtime/tools/builtins/agent-control.ts).
// The runtime text (agent-control.txt) describes five actions over runtime
// child sessions; the terminal's agents are Ink tasks, so this text says what
// each action does here and where `close` lives instead.
import { AGENT_CONTROL_TOOL_ID } from '@/runtime/primitives/wave-b-types'
import { TASK_STOP_TOOL_NAME } from '../TaskStopTool/prompt.js'

export const AGENT_CONTROL_TOOL_NAME = AGENT_CONTROL_TOOL_ID

export const DESCRIPTION = 'List, interrupt, resume or send a followup to your background agents'

export const PROMPT = `Manage the background agents you started with the Agent tool (run_in_background: true).

Actions:
- list: your agents, with id, name path, status, type and what each one is doing. \`path_prefix\` narrows it (for example "/root/tests").
- interrupt: stop an agent's current turn but keep its history. It shows as interrupted, not failed.
- resume: start an interrupted (or stopped) agent again where it stopped.
- followup: send \`message\` to an agent. A running agent reads it at its next step; a stopped or finished one starts a new turn in the background with its history. Use it to correct course or give the next piece of work to an agent that already has the context.

To end an agent for good, use ${TASK_STOP_TOOL_NAME} with its id.

\`target\` is an agent id or a name path such as "/root/tests"; every action but list needs it. \`reason\` (interrupt) is shown to the user.

After a followup or resume, use wait_agents to wait for the result instead of polling.`
