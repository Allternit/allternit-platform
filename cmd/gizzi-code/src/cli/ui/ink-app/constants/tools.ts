// @ts-nocheck
// biome-ignore-all assist/source/organizeImports: ANT-ONLY import markers must not be reordered
import { TASK_OUTPUT_TOOL_NAME } from '../tools/TaskOutputTool/constants.js'
import { EXIT_PLAN_MODE_V2_TOOL_NAME } from '../tools/ExitPlanModeTool/constants.js'
import { ENTER_PLAN_MODE_TOOL_NAME } from '../tools/EnterPlanModeTool/constants.js'
import { AGENT_TOOL_NAME } from '../tools/AgentTool/constants.js'
import { ASK_USER_QUESTION_TOOL_NAME } from '../tools/AskUserQuestionTool/prompt.js'
import { TASK_STOP_TOOL_NAME } from '../tools/TaskStopTool/prompt.js'
import { FILE_READ_TOOL_NAME } from '../tools/FileReadTool/prompt.js'
import { WEB_SEARCH_TOOL_NAME } from '../tools/WebSearchTool/prompt.js'
import { TODO_WRITE_TOOL_NAME } from '../tools/TodoWriteTool/constants.js'
import { GREP_TOOL_NAME } from '../tools/GrepTool/prompt.js'
import { WEB_FETCH_TOOL_NAME } from '../tools/WebFetchTool/prompt.js'
import { GLOB_TOOL_NAME } from '../tools/GlobTool/prompt.js'
import { SHELL_TOOL_NAMES } from '../utils/shell/shellToolUtils.js'
import { FILE_EDIT_TOOL_NAME } from '../tools/FileEditTool/constants.js'
import { FILE_WRITE_TOOL_NAME } from '../tools/FileWriteTool/prompt.js'
import { NOTEBOOK_EDIT_TOOL_NAME } from '../tools/NotebookEditTool/constants.js'
import { SKILL_TOOL_NAME } from '../tools/SkillTool/constants.js'
import { SEND_MESSAGE_TOOL_NAME } from '../tools/SendMessageTool/constants.js'
import { TASK_CREATE_TOOL_NAME } from '../tools/TaskCreateTool/constants.js'
import { TASK_GET_TOOL_NAME } from '../tools/TaskGetTool/constants.js'
import { TASK_LIST_TOOL_NAME } from '../tools/TaskListTool/constants.js'
import { TASK_UPDATE_TOOL_NAME } from '../tools/TaskUpdateTool/constants.js'
import { TOOL_SEARCH_TOOL_NAME } from '../tools/ToolSearchTool/prompt.js'
import { SYNTHETIC_OUTPUT_TOOL_NAME } from '../tools/SyntheticOutputTool/SyntheticOutputTool.js'
import { ENTER_WORKTREE_TOOL_NAME } from '../tools/EnterWorktreeTool/constants.js'
import { EXIT_WORKTREE_TOOL_NAME } from '../tools/ExitWorktreeTool/constants.js'
import { CURRENT_TIME_TOOL_NAME } from '../tools/CurrentTimeTool/prompt.js'
import { LOOKUP_TOOL_NAME } from '../tools/LookupTool/prompt.js'
import { THREAD_READ_TOOL_NAME, THREAD_RECENT_TOOL_NAME, THREAD_WAIT_TOOL_NAME } from '../tools/ThreadReadTool/prompt.js'
import { RESOURCE_TOOL_NAME } from '../tools/ResourceTool/prompt.js'
import { SEND_USER_MESSAGE_TOOL_NAME } from '../tools/SendUserMessageTool/prompt.js'
import { AUTOMATION_TOOL_NAME } from '../tools/AutomationTool/prompt.js'
import { ASK_USER_LATER_TOOL_NAME } from '../tools/AskUserLaterTool/prompt.js'
import { WAIT_AGENTS_TOOL_NAME } from '../tools/WaitAgentsTool/prompt.js'
import { AGENT_CONTROL_TOOL_NAME } from '../tools/AgentControlTool/prompt.js'
import { GET_CONTEXT_REMAINING_TOOL_NAME } from '../tools/GetContextRemainingTool/prompt.js'
import { AGENT_BATCH_TOOL_NAME } from '../tools/AgentBatchTool/prompt.js'

export const ALL_AGENT_DISALLOWED_TOOLS = new Set([
  TASK_OUTPUT_TOOL_NAME,
  EXIT_PLAN_MODE_V2_TOOL_NAME,
  ENTER_PLAN_MODE_TOOL_NAME,
  // Allow Agent tool for agents when user is ant (enables nested agents)
  ...(process.env.USER_TYPE === 'ant' ? [] : [AGENT_TOOL_NAME]),
  ASK_USER_QUESTION_TOOL_NAME,
  TASK_STOP_TOOL_NAME,
  // Wave B: these act on the main thread's agents (subagents start none).
  WAIT_AGENTS_TOOL_NAME,
  AGENT_CONTROL_TOOL_NAME,
  // A batch fans out one worker per row: only the main thread runs one, so a
  // subagent (or a batch worker) never starts another batch.
  AGENT_BATCH_TOOL_NAME,
])

export const CUSTOM_AGENT_DISALLOWED_TOOLS = new Set([
  ...ALL_AGENT_DISALLOWED_TOOLS,
])

/*
 * Async Agent Tool Availability Status (Source of Truth)
 */
export const ASYNC_AGENT_ALLOWED_TOOLS = new Set([
  FILE_READ_TOOL_NAME,
  WEB_SEARCH_TOOL_NAME,
  TODO_WRITE_TOOL_NAME,
  GREP_TOOL_NAME,
  WEB_FETCH_TOOL_NAME,
  GLOB_TOOL_NAME,
  ...SHELL_TOOL_NAMES,
  FILE_EDIT_TOOL_NAME,
  FILE_WRITE_TOOL_NAME,
  NOTEBOOK_EDIT_TOOL_NAME,
  SKILL_TOOL_NAME,
  SYNTHETIC_OUTPUT_TOOL_NAME,
  TOOL_SEARCH_TOOL_NAME,
  ENTER_WORKTREE_TOOL_NAME,
  EXIT_WORKTREE_TOOL_NAME,
  // Workers and background agents can read the user's clock, look up a fact and hand the user a deliverable.
  CURRENT_TIME_TOOL_NAME,
  LOOKUP_TOOL_NAME,
  THREAD_READ_TOOL_NAME,
  THREAD_RECENT_TOOL_NAME,
  THREAD_WAIT_TOOL_NAME,
  RESOURCE_TOOL_NAME,
  SEND_USER_MESSAGE_TOOL_NAME,
  // A question that doesn't block: a worker's ask lands in the main session's thread.
  ASK_USER_LATER_TOOL_NAME,
  // history_recall stays main-thread: it reads the main session's transcript.
  GET_CONTEXT_REMAINING_TOOL_NAME,
])
/**
 * Tools allowed only for in-process teammates (not general async agents).
 * These are injected by inProcessRunner.ts and allowed through filterToolsForAgent
 * via isInProcessTeammate() check.
 */
export const IN_PROCESS_TEAMMATE_ALLOWED_TOOLS = new Set([
  TASK_CREATE_TOOL_NAME,
  TASK_GET_TOOL_NAME,
  TASK_LIST_TOOL_NAME,
  TASK_UPDATE_TOOL_NAME,
  SEND_MESSAGE_TOOL_NAME,
  // A teammate's this_thread scheduled tasks are tagged with its agentId and
  // routed to its pendingUserMessages queue (see useScheduledTasks.ts).
  AUTOMATION_TOOL_NAME,
])

/*
 * BLOCKED FOR ASYNC AGENTS:
 * - AgentTool: Blocked to prevent recursion
 * - TaskOutputTool: Blocked to prevent recursion
 * - ExitPlanModeTool: Plan mode is a main thread abstraction.
 * - TaskStopTool: Requires access to main thread task state.
 * - TungstenTool: Uses singleton virtual terminal abstraction that conflicts between agents.
 * - folder_access: Always asks the user; async agents have nobody to ask, so it is left out.
 *
 * ENABLE LATER (NEED WORK):
 * - MCPTool: TBD
 * - ListMcpResourcesTool: TBD
 * - ReadMcpResourceTool: TBD
 */

/**
 * Tools allowed in coordinator mode - only output and agent management tools for the coordinator
 */
export const COORDINATOR_MODE_ALLOWED_TOOLS = new Set([
  AGENT_TOOL_NAME,
  TASK_STOP_TOOL_NAME,
  SEND_MESSAGE_TOOL_NAME,
  SYNTHETIC_OUTPUT_TOOL_NAME,
  WAIT_AGENTS_TOOL_NAME,
  AGENT_CONTROL_TOOL_NAME,
])
