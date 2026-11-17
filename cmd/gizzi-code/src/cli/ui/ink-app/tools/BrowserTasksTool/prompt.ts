// Same name and description as the runtime builtin (runtime/tools/builtins/browser-tasks.ts).
import BROWSER_TASKS_TXT from '@/runtime/tools/builtins/browser-tasks.txt'

export const BROWSER_TASKS_TOOL_NAME = 'browser_tasks'

export const DESCRIPTION = 'Run up to 6 hidden browser tasks in parallel on this machine'

export const PROMPT: string = BROWSER_TASKS_TXT
