// Same names and descriptions as the runtime builtins (runtime/tools/builtins/thread-read.ts).
import THREAD_READ_TXT from '@/runtime/tools/builtins/thread-read.txt'

export const THREAD_READ_TOOL_NAME = 'thread_read'
export const THREAD_RECENT_TOOL_NAME = 'thread_recent'
export const THREAD_WAIT_TOOL_NAME = 'thread_wait'

export const THREAD_READ_PROMPT: string = THREAD_READ_TXT
export const THREAD_RECENT_PROMPT = `List the user's most recent chats (threads), newest first, with title, status, goal, summary and last activity. Filter by time (before / after) or project. Use thread_read to open one.`
export const THREAD_WAIT_PROMPT = `Wait for one of up to 8 threads to finish a turn or need the user. Returns the first that does; on timeout, the latest status line of each. timeout_s 0 is a snapshot. Pass each target's cursor back as after_cursor so the same event is not returned twice.`
