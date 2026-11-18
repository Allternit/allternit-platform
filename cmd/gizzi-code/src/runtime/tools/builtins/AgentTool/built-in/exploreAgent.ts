import { BASH_TOOL_NAME } from '@/tools/BashTool/toolName.js'
import { EXIT_PLAN_MODE_TOOL_NAME } from '@/tools/ExitPlanModeTool/constants.js'
import { FILE_EDIT_TOOL_NAME } from '@/tools/FileEditTool/constants.js'
import { FILE_READ_TOOL_NAME } from '@/tools/FileReadTool/prompt.js'
import { FILE_WRITE_TOOL_NAME } from '@/tools/FileWriteTool/prompt.js'
import { GLOB_TOOL_NAME } from '@/tools/GlobTool/prompt.js'
import { GREP_TOOL_NAME } from '@/tools/GrepTool/prompt.js'
import { NOTEBOOK_EDIT_TOOL_NAME } from '@/tools/NotebookEditTool/constants.js'
import { hasEmbeddedSearchTools } from '@/utils/embeddedTools.js'
import { AGENT_TOOL_NAME } from '../constants.js'
import type { BuiltInAgentDefinition } from '../loadAgentsDir.js'

function getExploreSystemPrompt(): string {
  // Builds with embedded search alias find/grep in the shell and drop the
  // dedicated Glob/Grep tools, so point at find/grep via Bash instead.
  const embedded = hasEmbeddedSearchTools()
  const globGuidance = embedded
    ? `- Find files by name pattern with \`find\` in ${BASH_TOOL_NAME}.`
    : `- Find files by name pattern with ${GLOB_TOOL_NAME}.`
  const grepGuidance = embedded
    ? `- Search file contents with \`grep\` in ${BASH_TOOL_NAME}.`
    : `- Search file contents with ${GREP_TOOL_NAME}.`

  return `You are Explore, the read-only search agent of Gizzi Code. You find things in a codebase and report what you found.

Read-only. You change nothing: no new, edited, moved, copied or deleted files, no temp files (not even in /tmp), no write redirects (>, >>, tee) or heredocs, no installs, no git writes, no command that changes state. Edit tools are not available to you.

Method:
${globGuidance}
${grepGuidance}
- Use ${FILE_READ_TOOL_NAME} for a path you already know; read only the part you need.
- Use ${BASH_TOOL_NAME} only to look: ls, git status, git log, git diff, find${embedded ? ', grep' : ''}, cat, head, tail.
- Be fast. Send independent searches and reads together in one turn.
- Match depth to the thoroughness the caller asked for (quick, medium, very thorough). Before saying something does not exist, try other names, spellings and locations.

Report: your final message is the answer. Lead with the conclusion, then the evidence as absolute file paths with line numbers. No file dumps, no restating the task.`
}

export const EXPLORE_AGENT_MIN_QUERIES = 3

const EXPLORE_WHEN_TO_USE =
  'Fast read-only search: find files by pattern, find code by keyword, or answer how part of the codebase works. Returns conclusions with file:line, not file dumps. Say how thorough to be: "quick", "medium" or "very thorough". For one known file or symbol, search directly instead.'

// TODO(types): the runtime loadAgentsDir.ts BuiltInAgentDefinition mirror
// omits the disallowedTools/model/omitClaudeMd fields this record carries
// (cf. verificationAgent.ts sibling pattern).
type ExploreAgentDefinition = BuiltInAgentDefinition & {
  disallowedTools?: string[]
  model?: string
  omitClaudeMd?: boolean
}

export const EXPLORE_AGENT: ExploreAgentDefinition = {
  agentType: 'Explore',
  whenToUse: EXPLORE_WHEN_TO_USE,
  disallowedTools: [
    AGENT_TOOL_NAME,
    EXIT_PLAN_MODE_TOOL_NAME,
    FILE_EDIT_TOOL_NAME,
    FILE_WRITE_TOOL_NAME,
    NOTEBOOK_EDIT_TOOL_NAME,
  ],
  source: 'built-in',
  baseDir: 'built-in',
  // Ants get inherit to use the main agent's model; external users get haiku for speed
  // Note: For ants, getAgentModel() checks tengu_explore_agent GrowthBook flag at runtime
  model: process.env.USER_TYPE === 'ant' ? 'inherit' : 'haiku',
  // Explore is a fast read-only search agent — it doesn't need commit/PR/lint
  // rules from the instruction file. The main agent has full context and interprets results.
  omitClaudeMd: true,
  getSystemPrompt: () => getExploreSystemPrompt(),
}
