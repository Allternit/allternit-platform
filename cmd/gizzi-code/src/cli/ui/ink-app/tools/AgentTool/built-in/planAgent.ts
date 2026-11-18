import { BASH_TOOL_NAME } from './../../BashTool/toolName.ts'
import { EXIT_PLAN_MODE_TOOL_NAME } from './../../ExitPlanModeTool/constants.ts'
import { FILE_EDIT_TOOL_NAME } from './../../FileEditTool/constants.ts'
import { FILE_READ_TOOL_NAME } from './../../FileReadTool/prompt.ts'
import { FILE_WRITE_TOOL_NAME } from './../../FileWriteTool/prompt.ts'
import { GLOB_TOOL_NAME } from './../../GlobTool/prompt.ts'
import { GREP_TOOL_NAME } from './../../GrepTool/prompt.ts'
import { NOTEBOOK_EDIT_TOOL_NAME } from './../../NotebookEditTool/constants.ts'
import { hasEmbeddedSearchTools } from './../../../utils/embeddedTools.ts'
import { AGENT_TOOL_NAME } from '../constants.js'
import type { BuiltInAgentDefinition } from '../loadAgentsDir.js'
import { EXPLORE_AGENT } from './exploreAgent.js'

function getPlanV2SystemPrompt(): string {
  // Builds with embedded search alias find/grep in the shell and drop the
  // dedicated Glob/Grep tools, so point at find/grep instead.
  const searchToolsHint = hasEmbeddedSearchTools()
    ? `\`find\`, \`grep\` and ${FILE_READ_TOOL_NAME}`
    : `${GLOB_TOOL_NAME}, ${GREP_TOOL_NAME} and ${FILE_READ_TOOL_NAME}`

  return `You are Plan, the read-only planning agent of Gizzi Code. You study the codebase and write an implementation plan that someone else will build without asking you anything.

Read-only. You change nothing: no new, edited, moved, copied or deleted files, no temp files, no write redirects or heredocs, no installs, no git writes. Edit tools are not available to you. Use ${BASH_TOOL_NAME} only to look (ls, git status, git log, git diff, find${hasEmbeddedSearchTools() ? ', grep' : ''}, cat, head, tail).

You get requirements and sometimes a perspective to design from. Keep that perspective throughout.

Method:
1. Read every file the caller named. Then explore with ${searchToolsHint}: the current architecture, the patterns and conventions in use, a similar feature to copy, and the code paths the change runs through. Send independent searches together.
2. Answer your own questions from the repo. Only a question the repo cannot answer (a product choice, a preference, access you lack) goes back to the caller, listed under "Open questions" with the option you recommend.
3. Choose one approach. Name the main alternative in one line and why you did not pick it. Follow existing patterns unless there is a stated reason not to.

Plan format (decision-complete: no step says "decide" or "investigate"):
- Goal and constraints, in two or three lines.
- Steps, in order. Each step gives: the files it changes (absolute or repo-relative paths), what to change in them, its done check (a command or observable result that proves the step works), and the model class to run it (small for search and checks, standard for normal builds, deep for risky work: four or more files, data migrations, auth, billing, or a bug with no known cause).
- Which steps can run in parallel. One owner per file: two parallel steps never touch the same file.
- Risks and how each is contained.
- Open questions, only if any remain.

End your response with:

### Critical Files for Implementation
List 3-5 files most critical for implementing this plan:
- path/to/file1.ts
- path/to/file2.ts
- path/to/file3.ts`
}

export const PLAN_AGENT: BuiltInAgentDefinition = {
  agentType: 'Plan',
  whenToUse:
    'Read-only planning agent. Use it to turn a goal into a decision-complete implementation plan: files, ordered steps, a done check and a model class per step, risks, and the critical files. Give it the goal, the constraints and what you already found.',
  disallowedTools: [
    AGENT_TOOL_NAME,
    EXIT_PLAN_MODE_TOOL_NAME,
    FILE_EDIT_TOOL_NAME,
    FILE_WRITE_TOOL_NAME,
    NOTEBOOK_EDIT_TOOL_NAME,
  ],
  source: 'built-in',
  tools: EXPLORE_AGENT.tools,
  baseDir: 'built-in',
  model: 'inherit',
  // Plan is read-only and can read the instruction file directly if it needs conventions.
  // Dropping it from context saves tokens without blocking access.
  omitClaudeMd: true,
  getSystemPrompt: () => getPlanV2SystemPrompt(),
}
