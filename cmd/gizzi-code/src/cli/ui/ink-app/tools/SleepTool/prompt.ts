// Same name and description as the runtime builtin (runtime/tools/builtins/sleep.ts).
// SLEEP_TOOL_NAME is also read by query.ts (a queued message joins the same
// turn after a sleep), REPL.tsx and the auto-mode allowlist.
import SLEEP_TXT from '@/runtime/tools/builtins/sleep.txt'

export const SLEEP_TOOL_NAME = 'sleep'

export const DESCRIPTION = 'Wait a short time inside this turn; wakes early when the user writes'

export const PROMPT: string = SLEEP_TXT

/** @deprecated The old PROACTIVE/KAIROS stub's name for the prompt. */
export const SLEEP_TOOL_PROMPT = PROMPT
