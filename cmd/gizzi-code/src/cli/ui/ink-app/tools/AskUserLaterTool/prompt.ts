// Same name and description as the runtime builtin (runtime/tools/builtins/ask-user-later.ts).
import ASK_USER_LATER_TXT from '@/runtime/tools/builtins/ask-user-later.txt'

export const ASK_USER_LATER_TOOL_NAME = 'ask_user_later'

export const DESCRIPTION = 'Ask the user something without stopping your work'

export const PROMPT: string = `${ASK_USER_LATER_TXT}
In this terminal the blocking question tool is AskUserQuestion. The user answers an ask here with /answer <id>, or from the Allternit app when this session is live.`
