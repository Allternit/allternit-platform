import { type as osType, version as osVersion, release as osRelease } from 'os'
import PROMPT_BASE_CODE from '@/runtime/session/prompt/base-code.txt'
import PROMPT_SAFETY from '@/runtime/session/prompt/safety.txt'
import PROMPT_MODE_AUTO from '@/runtime/session/prompt/mode-auto.txt'
import PROMPT_MODE_ASK from '@/runtime/session/prompt/mode-ask.txt'
import type { PermissionMode } from '../types/permissions.js'
import { readGizziEnv } from '@/shared/utils/gizziEnv.js';
import { env } from '../utils/env.js'
import { getIsGit } from '../utils/git.js'
import { getCwd } from '../utils/cwd.js'
import { getIsNonInteractiveSession } from '../bootstrap/state.js'
import { getCurrentWorktreeSession } from '../utils/worktree.js'
import { getSessionStartDate } from './common.js'
import { getInitialSettings } from '../utils/settings/settings.js'
import {
  AGENT_TOOL_NAME,
  VERIFICATION_AGENT_TYPE,
} from '../tools/AgentTool/constants.js'
import { FILE_WRITE_TOOL_NAME } from '../tools/FileWriteTool/prompt.js'
import { FILE_READ_TOOL_NAME } from '../tools/FileReadTool/prompt.js'
import { FILE_EDIT_TOOL_NAME } from '../tools/FileEditTool/constants.js'
import { TODO_WRITE_TOOL_NAME } from '../tools/TodoWriteTool/constants.js'
import { TASK_CREATE_TOOL_NAME } from '../tools/TaskCreateTool/constants.js'
import { TASK_UPDATE_TOOL_NAME } from '../tools/TaskUpdateTool/constants.js'
import { TASK_OUTPUT_TOOL_NAME } from '../tools/TaskOutputTool/constants.js'
import { TASK_STOP_TOOL_NAME } from '../tools/TaskStopTool/prompt.js'
import { ENTER_PLAN_MODE_TOOL_NAME } from '../tools/EnterPlanModeTool/constants.js'
import { EXIT_PLAN_MODE_TOOL_NAME } from '../tools/ExitPlanModeTool/constants.js'
import { WEB_FETCH_TOOL_NAME } from '../tools/WebFetchTool/prompt.js'
import { WEB_SEARCH_TOOL_NAME } from '../tools/WebSearchTool/prompt.js'
import type { Tools } from '../Tool.js'
import type { Command } from '../types/command.js'
import { BASH_TOOL_NAME } from '../tools/BashTool/toolName.js'
import {
  getCanonicalName,
  getMarketingNameForModel,
} from '../utils/model/model.js'
import { getSkillToolCommands } from './../commands.ts'
import { SKILL_TOOL_NAME } from '../tools/SkillTool/constants.js'
import { getOutputStyleConfig } from './outputStyleConfig.js'
import type {
  MCPServerConnection,
  ConnectedMCPServer,
} from '../services/mcp/types.js'
import { GLOB_TOOL_NAME } from './../tools/GlobTool/prompt.ts'
import { GREP_TOOL_NAME } from './../tools/GrepTool/prompt.ts'
import { hasEmbeddedSearchTools } from './../utils/embeddedTools.ts'
import { ASK_USER_QUESTION_TOOL_NAME } from '../tools/AskUserQuestionTool/prompt.js'
import {
  EXPLORE_AGENT,
  EXPLORE_AGENT_MIN_QUERIES,
} from './../tools/AgentTool/built-in/exploreAgent.ts'
import { areExplorePlanAgentsEnabled } from './../tools/AgentTool/builtInAgents.ts'
import {
  isScratchpadEnabled,
  getScratchpadDir,
} from '../utils/permissions/filesystem.js'
import { isEnvTruthy } from '../utils/envUtils.js'
import { isReplModeEnabled } from '../tools/REPLTool/constants.js'
import { feature } from 'bun:bundle'
import { getFeatureValue_CACHED_MAY_BE_STALE } from './../services/analytics/growthbook.ts'
import { shouldUseGlobalCacheScope } from '../utils/betas.js'
import { isForkSubagentEnabled } from '../tools/AgentTool/forkSubagent.js'
import {
  systemPromptSection,
  DANGEROUS_uncachedSystemPromptSection,
  resolveSystemPromptSections,
} from './systemPromptSections.js'
import { SLEEP_TOOL_NAME } from '../tools/SleepTool/prompt.js'
import { TICK_TAG } from './xml.js'
import { logForDebugging } from '../utils/debug.js'
import { loadMemoryPrompt } from '../memdir/memdir.js'
import { isUndercover } from '../utils/undercover.js'
import { getAntModelOverrideConfig } from '../utils/model/antModels.js'
import { isMcpInstructionsDeltaEnabled } from '../utils/mcpInstructionsDelta.js'

// Feature-gated modules load only in builds that turn the feature on.
/* eslint-disable @typescript-eslint/no-require-imports */
const proactiveModule =
  feature('PROACTIVE') || feature('KAIROS')
    ? require('../proactive/index.js')
    : null
const BRIEF_PROACTIVE_SECTION: string | null =
  feature('KAIROS') || feature('KAIROS_BRIEF')
    ? (
        require('../tools/BriefTool/prompt.js') as typeof import('../tools/BriefTool/prompt.js')
      ).BRIEF_PROACTIVE_SECTION
    : null
const briefToolModule =
  feature('KAIROS') || feature('KAIROS_BRIEF')
    ? (require('../tools/BriefTool/BriefTool.js') as typeof import('../tools/BriefTool/BriefTool.js'))
    : null
/* eslint-enable @typescript-eslint/no-require-imports */
import type { OutputStyleConfig } from './outputStyles.js'

/**
 * Splits the cacheable static prefix from the per-session sections.
 * Everything before it may use the global cache scope; everything after is
 * user or session specific. Keep it in step with splitSysPromptPrefix in
 * utils/api.ts and buildSystemPromptBlocks in the API layer.
 */
export const SYSTEM_PROMPT_DYNAMIC_BOUNDARY =
  '__SYSTEM_PROMPT_DYNAMIC_BOUNDARY__'

const ISSUES_URL = 'https://github.com/Allternit/allternit-platform/issues'

export function prependBullets(items: Array<string | string[]>): string[] {
  return items.flatMap(item =>
    Array.isArray(item)
      ? item.map(subitem => `  - ${subitem}`)
      : [` - ${item}`],
  )
}

// --- Static TUI sections: appended after the runtime's base-code.txt and safety.txt ---

const SECURITY_WORK_LINE = `Security work: help with authorized testing, defense, CTFs and teaching. Decline destructive attacks, denial of service, mass targeting, supply-chain compromise, or evading detection to cause harm. Exploit code, C2 frameworks and credential-testing tools need a clear authorized context such as a pentest engagement, a CTF or defensive research.`

/**
 * The runtime code prompt. A non-coding output style drops its "Doing tasks"
 * section and points at the style instead, unless the style keeps coding
 * instructions. Every other section, care and git rules included, stays.
 */
function getCodePrompt(outputStyleConfig: OutputStyleConfig | null): string {
  const base = PROMPT_BASE_CODE.trim()
  if (
    outputStyleConfig === null ||
    outputStyleConfig.keepCodingInstructions === true
  ) {
    return base
  }
  return base
    .replace(/\n# Doing tasks\n[\s\S]*?(?=\n# )/, '')
    .replace('\n', '\nRespond the way the Output Style section below describes.\n')
}

function getToolNamesLine(): string {
  const names = [
    ['read', FILE_READ_TOOL_NAME],
    ['edit', FILE_EDIT_TOOL_NAME],
    ['write', FILE_WRITE_TOOL_NAME],
    ['grep', GREP_TOOL_NAME],
    ['glob', GLOB_TOOL_NAME],
    ['bash', BASH_TOOL_NAME],
    ['task', AGENT_TOOL_NAME],
    ['todowrite', `${TODO_WRITE_TOOL_NAME} (or ${TASK_CREATE_TOOL_NAME} and ${TASK_UPDATE_TOOL_NAME} when offered)`],
    ['task_output', TASK_OUTPUT_TOOL_NAME],
    ['task_stop', TASK_STOP_TOOL_NAME],
    ['question', ASK_USER_QUESTION_TOOL_NAME],
    ['plan_enter', ENTER_PLAN_MODE_TOOL_NAME],
    ['plan_exit', EXIT_PLAN_MODE_TOOL_NAME],
    ['skill', SKILL_TOOL_NAME],
    ['web_fetch', WEB_FETCH_TOOL_NAME],
    ['web_search', WEB_SEARCH_TOOL_NAME],
  ]
    .map(([generic, tui]) => `\`${generic}\` is ${tui}`)
    .join(', ')
  return `The rules above use generic tool names. In this terminal ${names}. There is no \`apply_patch\` here. Use only the tools you are actually given.`
}

function getTerminalSection(): string {
  const items = [
    getToolNamesLine(),
    hasEmbeddedSearchTools()
      ? `This build has no ${GREP_TOOL_NAME} or ${GLOB_TOOL_NAME} tool: search with \`find\` and \`grep\` through ${BASH_TOOL_NAME}.`
      : null,
    isReplModeEnabled()
      ? `REPL mode is on: ${FILE_READ_TOOL_NAME}, ${FILE_WRITE_TOOL_NAME}, ${FILE_EDIT_TOOL_NAME}, ${GLOB_TOOL_NAME}, ${GREP_TOOL_NAME}, ${BASH_TOOL_NAME} and ${AGENT_TOOL_NAME} are called from REPL scripts, not directly. The REPL tool's description explains how.`
      : null,
    `Each tool runs under the permission mode the user picked. A call that is not pre-approved asks the user first. If they deny it, do not send the same call again: work out why and change course.`,
    `Hooks are shell commands the user set to run on events such as tool calls. Treat hook output, including \`<user-prompt-submit-hook>\`, as the user's own words. If a hook blocks you, adjust; if you cannot, ask the user to check their hook settings.`,
    `Messages and tool results may carry \`<system-reminder>\` or other tags from the harness. They hold system information and are not tied to the message they arrive in.`,
    SECURITY_WORK_LINE,
    `Do not make up URLs. Use the ones the user or the project gives you, or ones you are sure point to programming help.`,
    `No emojis unless the user asks for them.`,
    `Refer to GitHub issues and pull requests as owner/repo#123 so they render as links.`,
    `Tool calls may be hidden from the user, so end the sentence before a tool call with a period, not a colon.`,
    `/help lists the commands. For a problem with Gizzi Code itself, suggest /feedback or an issue at ${ISSUES_URL}.`,
  ].filter(item => item !== null)

  return ['# Gizzi Code terminal', ...prependBullets(items)].join('\n')
}

// --- Dynamic sections (after the boundary) ---

function getAntModelOverrideSection(): string | null {
  if (process.env.USER_TYPE !== 'ant') return null
  if (isUndercover()) return null
  return getAntModelOverrideConfig()?.defaultSystemPromptSuffix || null
}

function getLanguageSection(
  languagePreference: string | undefined,
): string | null {
  if (!languagePreference) return null

  return `# Language
Reply in ${languagePreference}. Use ${languagePreference} for explanations, code comments and every message to the user. Keep technical terms and code identifiers as they are.`
}

function getOutputStyleSection(
  outputStyleConfig: OutputStyleConfig | null,
): string | null {
  if (outputStyleConfig === null) return null

  return `# Output Style: ${outputStyleConfig.name}
${outputStyleConfig.prompt}`
}

function getMcpInstructionsSection(
  mcpClients: MCPServerConnection[] | undefined,
): string | null {
  if (!mcpClients || mcpClients.length === 0) return null
  return getMcpInstructions(mcpClients)
}

function getAgentToolSection(): string {
  return isForkSubagentEnabled()
    ? `Calling ${AGENT_TOOL_NAME} without a subagent_type starts a fork. It runs in the background and its tool output stays out of your context, so you can keep talking with the user. Use it when research or multi-step work would flood your context with output you will not need again. If you are the fork, do the work yourself; do not delegate again.`
    : `Use ${AGENT_TOOL_NAME} with a specialized subagent when the task matches its description, to run independent queries in parallel or to keep large results out of your context. Do not repeat searches you handed to a subagent.`
}

/**
 * Guidance that depends on this session's tools and mode. It sits after the
 * boundary so each runtime bit does not split the cached static prefix.
 */
function getSessionSpecificGuidanceSection(
  enabledTools: Set<string>,
  skillToolCommands: Command[],
): string | null {
  const hasAskUserQuestionTool = enabledTools.has(ASK_USER_QUESTION_TOOL_NAME)
  const hasSkills =
    skillToolCommands.length > 0 && enabledTools.has(SKILL_TOOL_NAME)
  const hasAgentTool = enabledTools.has(AGENT_TOOL_NAME)
  const searchTools = hasEmbeddedSearchTools()
    ? `\`find\` or \`grep\` through ${BASH_TOOL_NAME}`
    : `${GLOB_TOOL_NAME} or ${GREP_TOOL_NAME}`

  const items = [
    hasAskUserQuestionTool
      ? `If you cannot tell why the user denied a tool call, ask with ${ASK_USER_QUESTION_TOOL_NAME}.`
      : null,
    getIsNonInteractiveSession()
      ? null
      : `If the user must run a command themselves (an interactive login such as \`gcloud auth login\`), tell them to type \`! <command>\` at the prompt. It runs in this session and its output joins the conversation.`,
    // isForkSubagentEnabled() reads the session type, so it stays after the boundary.
    hasAgentTool ? getAgentToolSection() : null,
    ...(hasAgentTool &&
    areExplorePlanAgentsEnabled() &&
    !isForkSubagentEnabled()
      ? [
          `For a directed search (a known file, class or function) use ${searchTools} directly. Use ${AGENT_TOOL_NAME} with subagent_type=${EXPLORE_AGENT.agentType} only when a direct search is not enough or the task clearly needs more than ${EXPLORE_AGENT_MIN_QUERIES} queries; it is slower.`,
        ]
      : []),
    hasSkills
      ? `\`/<skill-name>\` (for example /commit) is how the user invokes a skill; it expands into a full prompt. Run it with ${SKILL_TOOL_NAME}, and only for skills listed in that tool's user-invocable section. Never guess skill names or call built-in commands through it.`
      : null,
    hasAgentTool &&
    feature('VERIFICATION_AGENT') &&
    getFeatureValue_CACHED_MAY_BE_STALE('tengu_hive_evidence', false)
      ? `When your turn includes non-trivial implementation (three or more files edited, backend or API changes, or infrastructure changes), get an independent check before you report done, whoever wrote the code: you, a fork or a subagent. Spawn ${AGENT_TOOL_NAME} with subagent_type="${VERIFICATION_AGENT_TYPE}" and pass the original request, every changed file, the approach and the plan file path if there is one. Share concerns, but not test results or claims that it works. Only the verifier gives the verdict; your own checks do not replace it. On FAIL: fix, resume the verifier with its findings and your fix, and repeat until PASS. On PASS: rerun two or three of its commands and confirm each PASS shows a command with output that matches; if not, resume it with the details. On PARTIAL: report what passed and what could not be checked.`
      : null,
  ].filter(item => item !== null)

  if (items.length === 0) return null
  return ['# Session guidance', ...prependBullets(items)].join('\n')
}

function getSystemRemindersSection(): string {
  return `- Messages and tool results may carry \`<system-reminder>\` tags from the harness. They hold system information and are not tied to the message they arrive in.
- Earlier history is summarized automatically, so the conversation is not limited by the context window.`
}

/**
 * The autonomy block for this terminal's permission mode, the same blocks the
 * runtime picks (system.ts SystemPrompt.autonomy): acceptEdits, auto, dontAsk
 * and bypassPermissions act on routine steps (mode-auto.txt); default asks
 * first (mode-ask.txt). Plan has its own plan-mode attachments and bubble (a
 * subagent asking its parent) follows the parent, so both get none. With no
 * mode passed, the settings' defaultMode stands in.
 */
export function getPermissionModeSection(
  mode: PermissionMode | undefined,
): string | null {
  const effective =
    mode ??
    (getInitialSettings().permissions?.defaultMode as PermissionMode | undefined) ??
    'default'
  switch (effective) {
    case 'acceptEdits':
    case 'auto':
    case 'dontAsk':
    case 'bypassPermissions':
      return PROMPT_MODE_AUTO.trim()
    case 'default':
      return PROMPT_MODE_ASK.trim()
    default:
      return null
  }
}

const SUMMARIZE_TOOL_RESULTS_SECTION = `Tool results may be cleared from context later. Write down in your reply anything from them you will need.`

/**
 * The default TUI system prompt. It starts from the runtime's code prompt
 * (base-code.txt) and safety rules (safety.txt), the same text Code mode
 * sends, then adds only what is specific to this terminal.
 */
export async function getSystemPrompt(
  tools: Tools,
  model: string,
  additionalWorkingDirectories?: string[],
  mcpClients?: MCPServerConnection[],
  /** The session's permission mode (toolPermissionContext.mode); picks mode-auto or mode-ask. */
  permissionMode?: PermissionMode,
): Promise<string[]> {
  if (isEnvTruthy(readGizziEnv('SIMPLE'))) {
    return [
      `You are Gizzi Code, Allternit's coding agent.\n\nCWD: ${getCwd()}\nDate: ${getSessionStartDate()}`,
    ]
  }

  const cwd = getCwd()
  const [skillToolCommands, outputStyleConfig, envInfo] = await Promise.all([
    getSkillToolCommands(cwd),
    getOutputStyleConfig(),
    computeSimpleEnvInfo(model, additionalWorkingDirectories),
  ])

  const settings = getInitialSettings()
  const enabledTools = new Set(tools.map(_ => _.name))

  if (
    (feature('PROACTIVE') || feature('KAIROS')) &&
    proactiveModule?.isProactiveActive()
  ) {
    logForDebugging(`[SystemPrompt] path=simple-proactive`)
    return [
      `You are Gizzi Code, Allternit's coding agent, running on your own. Use the tools you have to do useful work.\n\n${SECURITY_WORK_LINE}`,
      PROMPT_SAFETY,
      getSystemRemindersSection(),
      await loadMemoryPrompt(),
      envInfo,
      getLanguageSection(settings.language),
      // With the delta on, MCP instructions arrive as mcp_instructions_delta attachments instead.
      isMcpInstructionsDeltaEnabled()
        ? null
        : getMcpInstructionsSection(mcpClients),
      getScratchpadInstructions(),
      SUMMARIZE_TOOL_RESULTS_SECTION,
      getProactiveSection(),
    ].filter(s => s !== null)
  }

  const dynamicSections = [
    systemPromptSection('session_guidance', () =>
      getSessionSpecificGuidanceSection(enabledTools, skillToolCommands),
    ),
    // Shift+Tab, /permissions and plan exits change the mode between turns,
    // so it is read each turn; the text only changes when the mode does.
    DANGEROUS_uncachedSystemPromptSection(
      'permission_mode',
      () => getPermissionModeSection(permissionMode),
      'the permission mode changes between turns',
    ),
    systemPromptSection('memory', () => loadMemoryPrompt()),
    systemPromptSection('ant_model_override', () =>
      getAntModelOverrideSection(),
    ),
    systemPromptSection('env_info_simple', () =>
      computeSimpleEnvInfo(model, additionalWorkingDirectories),
    ),
    systemPromptSection('language', () =>
      getLanguageSection(settings.language),
    ),
    systemPromptSection('output_style', () =>
      getOutputStyleSection(outputStyleConfig),
    ),
    // With the delta on, MCP instructions arrive as persisted
    // mcp_instructions_delta attachments instead of this per-turn recompute,
    // which would bust the cache when a server connects late. The gate is
    // read inside compute so a mid-session flip never serves a stale value.
    DANGEROUS_uncachedSystemPromptSection(
      'mcp_instructions',
      () =>
        isMcpInstructionsDeltaEnabled()
          ? null
          : getMcpInstructionsSection(mcpClients),
      'MCP servers connect/disconnect between turns',
    ),
    systemPromptSection('scratchpad', () => getScratchpadInstructions()),
    ...(feature('TOKEN_BUDGET')
      ? [
          // Cached on purpose: the text is a no-op when no budget is set, and
          // first-response and continuation paths never see attachments.
          systemPromptSection(
            'token_budget',
            () =>
              'When the user sets a token target ("+500k", "spend 2M tokens"), your output token count is shown each turn. Keep doing useful work until you near the target; it is a minimum, not a suggestion. Stopping early triggers an automatic continue.',
          ),
        ]
      : []),
    ...(feature('KAIROS') || feature('KAIROS_BRIEF')
      ? [systemPromptSection('brief', () => getBriefSection())]
      : []),
  ]

  const resolvedDynamicSections =
    await resolveSystemPromptSections(dynamicSections)

  return [
    // --- Static content (cacheable) ---
    getCodePrompt(outputStyleConfig),
    PROMPT_SAFETY.trim(),
    getTerminalSection(),
    // === BOUNDARY MARKER - DO NOT MOVE OR REMOVE ===
    ...(shouldUseGlobalCacheScope() ? [SYSTEM_PROMPT_DYNAMIC_BOUNDARY] : []),
    // --- Dynamic content (registry-managed) ---
    ...resolvedDynamicSections,
  ].filter(s => s !== null)
}

function getMcpInstructions(mcpClients: MCPServerConnection[]): string | null {
  const connectedClients = mcpClients.filter(
    (client): client is ConnectedMCPServer => client.type === 'connected',
  )

  const clientsWithInstructions = connectedClients.filter(
    client => client.instructions,
  )

  if (clientsWithInstructions.length === 0) {
    return null
  }

  const instructionBlocks = clientsWithInstructions
    .map(client => {
      return `## ${client.name}
${client.instructions}`
    })
    .join('\n\n')

  return `# MCP Server Instructions

Connected MCP servers sent these instructions for their tools and resources:

${instructionBlocks}`
}

function getModelLine(modelId: string): string | null {
  // Internal undercover builds keep every model name out of the prompt.
  // The USER_TYPE check is inlined at each call site so external builds fold it away.
  if (process.env.USER_TYPE === 'ant' && isUndercover()) return null
  const marketingName = getMarketingNameForModel(modelId)
  return marketingName
    ? `You are powered by the model named ${marketingName}. The exact model ID is ${modelId}.`
    : `You are powered by the model ${modelId}.`
}

export async function computeEnvInfo(
  modelId: string,
  additionalWorkingDirectories?: string[],
): Promise<string> {
  const [isGit, unameSR] = await Promise.all([getIsGit(), getUnameSR()])

  const modelDescription = getModelLine(modelId) ?? ''

  const additionalDirsInfo =
    additionalWorkingDirectories && additionalWorkingDirectories.length > 0
      ? `Additional working directories: ${additionalWorkingDirectories.join(', ')}\n`
      : ''

  const cutoff = getKnowledgeCutoff(modelId)
  const knowledgeCutoffMessage = cutoff
    ? `\n\nAssistant knowledge cutoff is ${cutoff}.`
    : ''

  return `Here is useful information about the environment you are running in:
<env>
Working directory: ${getCwd()}
Is directory a git repo: ${isGit ? 'Yes' : 'No'}
${additionalDirsInfo}Platform: ${env.platform}
${getShellInfoLine()}
OS Version: ${unameSR}
</env>
${modelDescription}${knowledgeCutoffMessage}`
}

export async function computeSimpleEnvInfo(
  modelId: string,
  additionalWorkingDirectories?: string[],
): Promise<string> {
  const [isGit, unameSR] = await Promise.all([getIsGit(), getUnameSR()])

  const cutoff = getKnowledgeCutoff(modelId)
  const knowledgeCutoffMessage = cutoff
    ? `Assistant knowledge cutoff is ${cutoff}.`
    : null

  const cwd = getCwd()
  const isWorktree = getCurrentWorktreeSession() !== null

  const envItems = [
    `Primary working directory: ${cwd}`,
    isWorktree
      ? `This is a git worktree, an isolated copy of the repository. Run every command from here; do not \`cd\` to the original repository root.`
      : null,
    [`Is a git repository: ${isGit}`],
    additionalWorkingDirectories && additionalWorkingDirectories.length > 0
      ? `Additional working directories:`
      : null,
    additionalWorkingDirectories && additionalWorkingDirectories.length > 0
      ? additionalWorkingDirectories
      : null,
    `Platform: ${env.platform}`,
    getShellInfoLine(),
    `OS Version: ${unameSR}`,
    getModelLine(modelId),
    knowledgeCutoffMessage,
    `Fast mode (/fast) gives faster output from the same model; it does not switch models.`,
  ].filter(item => item !== null)

  return [
    `# Environment`,
    `You have been invoked in the following environment: `,
    ...prependBullets(envItems),
  ].join(`\n`)
}

function getKnowledgeCutoff(modelId: string): string | null {
  const canonical = getCanonicalName(modelId)
  if (canonical.includes('claude-sonnet-4-6')) {
    return 'August 2025'
  } else if (canonical.includes('claude-opus-4-6')) {
    return 'May 2025'
  } else if (canonical.includes('claude-opus-4-5')) {
    return 'May 2025'
  } else if (canonical.includes('claude-haiku-4')) {
    return 'February 2025'
  } else if (
    canonical.includes('claude-opus-4') ||
    canonical.includes('claude-sonnet-4')
  ) {
    return 'January 2025'
  }
  return null
}

function getShellInfoLine(): string {
  const shell = process.env.SHELL || 'unknown'
  const shellName = shell.includes('zsh')
    ? 'zsh'
    : shell.includes('bash')
      ? 'bash'
      : shell
  if (env.platform === 'win32') {
    return `Shell: ${shellName} (use Unix shell syntax, not Windows: /dev/null not NUL, forward slashes in paths)`
  }
  return `Shell: ${shellName}`
}

export function getUnameSR(): string {
  // os.type() and os.release() wrap uname(3) on POSIX, matching `uname -sr`
  // ("Darwin 25.3.0"). Windows has no uname; os.version() gives the
  // friendlier "Windows 11 Pro" there.
  if (env.platform === 'win32') {
    return `${osVersion()} ${osRelease()}`
  }
  return `${osType()} ${osRelease()}`
}

export const DEFAULT_AGENT_PROMPT = `You are a worker agent for Gizzi Code, Allternit's coding agent. Use your tools to finish the task you were given: completely, but without extras. When you are done, reply with a short report of what you did and what you found. The caller passes it on to the user, so include only what matters.`

export async function enhanceSystemPromptWithEnvDetails(
  existingSystemPrompt: string[],
  model: string,
  additionalWorkingDirectories?: string[],
  enabledToolNames?: ReadonlySet<string>,
): Promise<string[]> {
  const notes = `Notes:
- Your working directory resets between bash calls, so use absolute file paths.
- In your final report, give the relevant files as absolute paths. Quote code only when the exact text matters (a bug you found, a signature the caller asked for); do not recap code you only read.
- No emojis.
- End the sentence before a tool call with a period, not a colon.`
  const envInfo = await computeEnvInfo(model, additionalWorkingDirectories)
  return [
    ...existingSystemPrompt,
    notes,
    envInfo,
  ]
}

/** Scratchpad instructions, when the per-session scratchpad directory is enabled. */
export function getScratchpadInstructions(): string | null {
  if (!isScratchpadEnabled()) {
    return null
  }

  const scratchpadDir = getScratchpadDir()

  return `# Scratchpad Directory

Put temporary files in this session's scratchpad, not in \`/tmp\` or another system temp directory:
\`${scratchpadDir}\`

Use it for intermediate results, throwaway scripts and config, outputs that do not belong in the project, and anything else that would go in \`/tmp\`. Use \`/tmp\` only when the user asks for it. The scratchpad belongs to this session, sits outside the project, and needs no permission prompts.`
}

function getBriefSection(): string | null {
  if (!(feature('KAIROS') || feature('KAIROS_BRIEF'))) return null
  if (!BRIEF_PROACTIVE_SECTION) return null
  // When the tool is available the model is told to use it; /brief and
  // --brief only control the display filter.
  if (!briefToolModule?.isBriefEnabled()) return null
  // The proactive section already carries it inline.
  if (
    (feature('PROACTIVE') || feature('KAIROS')) &&
    proactiveModule?.isProactiveActive()
  )
    return null
  return BRIEF_PROACTIVE_SECTION
}

function getProactiveSection(): string | null {
  if (!(feature('PROACTIVE') || feature('KAIROS'))) return null
  if (!proactiveModule?.isProactiveActive()) return null

  return `# Autonomous work

You run on your own. \`<${TICK_TAG}>\` messages keep you awake between turns; read each one as "you are awake, what next?". The time in a tick is the user's local time. Outside tools (GitHub, chat apps) may report other time zones. Several ticks can arrive in one message: act on the latest and never repeat tick text.

## Pacing

Use ${SLEEP_TOOL_NAME} to set how long to wait: longer while slow work runs, shorter while you iterate. Each wake-up is a model call, and the prompt cache expires after five idle minutes, so weigh both.

**If a tick brings nothing useful to do, call ${SLEEP_TOOL_NAME} right away.** Write nothing. Never send "still waiting" or "nothing to do" messages.

## First tick

On the first tick of a new session, greet the user in one line and ask what to work on. Do not explore or change anything until they answer.

## Later ticks

Look for useful work: what is still unknown, what could break, what needs checking before this counts as done. Do not repeat a question the user has not answered yet. Do not narrate; act.

## Staying responsive

When the user is active, answer their messages promptly and keep the loop tight, ahead of background work.

## Acting

Use your own judgment instead of asking.

- Read, search, explore the project, run tests, typecheck and lint without asking.
- Make code changes, and commit at good stopping points.
- Between two reasonable approaches, pick one. You can correct course later.

## Reporting

Keep text short. Write only for decisions that need the user, status at real milestones ("PR opened", "tests passing"), and errors or blockers that change the plan. No step-by-step narration and no lists of files you read.

## Terminal focus

The context may carry a \`terminalFocus\` field.
- **Unfocused**: the user is away. Act on your own, including commits and pushes, and pause only for irreversible or high-risk steps.
- **Focused**: the user is watching. Offer choices, ask before large changes, and keep output easy to follow.${BRIEF_PROACTIVE_SECTION && briefToolModule?.isBriefEnabled() ? `\n\n${BRIEF_PROACTIVE_SECTION}` : ''}`
}
