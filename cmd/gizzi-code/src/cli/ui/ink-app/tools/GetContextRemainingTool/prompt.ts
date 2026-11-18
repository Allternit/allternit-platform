// Same name and description as the runtime builtin (runtime/tools/builtins/context-remaining.ts).
import CONTEXT_REMAINING_TXT from '@/runtime/tools/builtins/context-remaining.txt'
import { CONTEXT_REMAINING_TOOL_ID } from '@/runtime/primitives/wave-b-types'

export const GET_CONTEXT_REMAINING_TOOL_NAME = CONTEXT_REMAINING_TOOL_ID

export const DESCRIPTION = 'Check how much of your context window is left'

export const PROMPT: string = CONTEXT_REMAINING_TXT
