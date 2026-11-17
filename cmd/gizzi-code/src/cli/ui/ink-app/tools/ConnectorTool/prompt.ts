// Same names and descriptions as the runtime builtins (runtime/tools/builtins/connector.ts).
import SEARCH_TXT from '@/runtime/tools/builtins/connector-search.txt'
import SUGGEST_TXT from '@/runtime/tools/builtins/connector-suggest.txt'

export const CONNECTOR_SEARCH_TOOL_NAME = 'connector_search'
export const CONNECTOR_SUGGEST_TOOL_NAME = 'connector_suggest'

export const CONNECTOR_SEARCH_PROMPT: string = SEARCH_TXT
export const CONNECTOR_SUGGEST_PROMPT: string = SUGGEST_TXT
