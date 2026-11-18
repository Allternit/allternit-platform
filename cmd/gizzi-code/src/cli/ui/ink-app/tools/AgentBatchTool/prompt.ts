// Same name and description as the runtime builtin (runtime/tools/builtins/agent-batch.ts),
// with the TUI's names for the agent tool and the default worker.
import AGENT_BATCH_TXT from '@/runtime/tools/builtins/agent-batch.txt'

export const AGENT_BATCH_TOOL_NAME = 'agent_batch'

export const DESCRIPTION = 'Run one subagent per CSV row and write their JSON replies to a results CSV'

export const PROMPT: string = AGENT_BATCH_TXT.replace('use agent_swarm or task instead', 'use the Agent tool instead').replace(
  'the worker agent (default "general")',
  'the worker agent (default "general-purpose")',
)
