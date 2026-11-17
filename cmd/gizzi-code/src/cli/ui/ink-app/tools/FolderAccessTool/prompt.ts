// Same name, description and prompt as the runtime builtin `folder_access`
// (runtime/tools/builtins/folder-access.txt), imported so the two cannot drift.
import FOLDER_ACCESS_TXT from '@/runtime/tools/builtins/folder-access.txt'

export const FOLDER_ACCESS_TOOL_NAME = 'folder_access'

// The runtime's description is the whole .txt; its first line is the summary.
export const DESCRIPTION: string = FOLDER_ACCESS_TXT.split('\n')[0].trim()

export const PROMPT: string = FOLDER_ACCESS_TXT
