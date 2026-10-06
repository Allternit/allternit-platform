import { join, normalize, sep } from 'path'
import { getProjectRoot } from '../../bootstrap/state.js'
import {
  buildMemoryPrompt,
  ensureMemoryDirExists,
} from '../../memdir/memdir.js'
import {
  getMemoryBaseDir,
  getMemoryDrivePath,
  isMemoryDriveActive,
} from '../../memdir/paths.js'
import { readFileSync } from 'fs'
import { getCwd } from '../../utils/cwd.js'
import { findCanonicalGitRoot } from '../../utils/git.js'
import { sanitizePath } from '../../utils/path.js'

// Persistent agent memory scope: 'user' (~/.claude/agent-memory/), 'project' (.claude/agent-memory/), or 'local' (.claude/agent-memory-local/)
export type AgentMemoryScope = 'user' | 'project' | 'local'

/**
 * Sanitize an agent type name for use as a directory name.
 * Replaces colons (invalid on Windows, used in plugin-namespaced agent
 * types like "my-plugin:my-agent") with dashes.
 */
function sanitizeAgentTypeForPath(agentType: string): string {
  return agentType.replace(/:/g, '-')
}

/** Drive topic folders accept only [a-z0-9-] names. */
function driveAgentSlug(agentType: string): string {
  const slug = agentType
    .toLowerCase()
    .replace(/[^a-z0-9-]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 64)
  return slug || 'agent'
}

/**
 * With the Memory Drive on, user-scope agent memory lives in the personal
 * drive at agents/<agent>/memory.md (synced, versioned, Dreamed). Project
 * and local scopes stay in the project on purpose.
 */
function driveAgentMemoryDir(agentType: string): string | undefined {
  if (!isMemoryDriveActive()) return undefined
  const drive = getMemoryDrivePath()
  if (!drive) return undefined
  return join(drive, 'agents', driveAgentSlug(agentType)) + sep
}

/**
 * Returns the local agent memory directory, which is project-specific and not checked into VCS.
 * When GIZZI_CODE_REMOTE_MEMORY_DIR is set, persists to the mount with project namespacing.
 * Otherwise, uses <cwd>/.claude/agent-memory-local/<agentType>/.
 */
function getLocalAgentMemoryDir(dirName: string): string {
  if (process.env.GIZZI_CODE_REMOTE_MEMORY_DIR) {
    return (
      join(
        process.env.GIZZI_CODE_REMOTE_MEMORY_DIR,
        'projects',
        sanitizePath(
          findCanonicalGitRoot(getProjectRoot()) ?? getProjectRoot(),
        ),
        'agent-memory-local',
        dirName,
      ) + sep
    )
  }
  return join(getCwd(), '.claude', 'agent-memory-local', dirName) + sep
}

/**
 * Returns the agent memory directory for a given agent type and scope.
 * - 'user' scope: <memoryBase>/agent-memory/<agentType>/
 * - 'project' scope: <cwd>/.claude/agent-memory/<agentType>/
 * - 'local' scope: see getLocalAgentMemoryDir()
 */
export function getAgentMemoryDir(
  agentType: string,
  scope: AgentMemoryScope,
): string {
  const dirName = sanitizeAgentTypeForPath(agentType)
  switch (scope) {
    case 'project':
      return join(getCwd(), '.claude', 'agent-memory', dirName) + sep
    case 'local':
      return getLocalAgentMemoryDir(dirName)
    case 'user':
      return (
        driveAgentMemoryDir(agentType) ??
        join(getMemoryBaseDir(), 'agent-memory', dirName) + sep
      )
  }
}

// Check if file is within an agent memory directory (any scope).
export function isAgentMemoryPath(absolutePath: string): boolean {
  // SECURITY: Normalize to prevent path traversal bypasses via .. segments
  const normalizedPath = normalize(absolutePath)
  const memoryBase = getMemoryBaseDir()

  // User scope: check memory base (may be custom dir or config home)
  if (normalizedPath.startsWith(join(memoryBase, 'agent-memory') + sep)) {
    return true
  }
  // User scope on the Memory Drive: <drive>/agents/
  const drive = isMemoryDriveActive() ? getMemoryDrivePath() : undefined
  if (drive && normalizedPath.startsWith(join(drive, 'agents') + sep)) {
    return true
  }

  // Project scope: always cwd-based (not redirected)
  if (
    normalizedPath.startsWith(join(getCwd(), '.claude', 'agent-memory') + sep)
  ) {
    return true
  }

  // Local scope: persisted to mount when GIZZI_CODE_REMOTE_MEMORY_DIR is set, otherwise cwd-based
  if (process.env.GIZZI_CODE_REMOTE_MEMORY_DIR) {
    if (
      normalizedPath.includes(sep + 'agent-memory-local' + sep) &&
      normalizedPath.startsWith(
        join(process.env.GIZZI_CODE_REMOTE_MEMORY_DIR, 'projects') + sep,
      )
    ) {
      return true
    }
  } else if (
    normalizedPath.startsWith(
      join(getCwd(), '.claude', 'agent-memory-local') + sep,
    )
  ) {
    return true
  }

  return false
}

/**
 * Returns the agent memory file path for a given agent type and scope.
 */
export function getAgentMemoryEntrypoint(
  agentType: string,
  scope: AgentMemoryScope,
): string {
  return join(getAgentMemoryDir(agentType, scope), 'MEMORY.md')
}

export function getMemoryScopeDisplay(
  memory: AgentMemoryScope | undefined,
): string {
  switch (memory) {
    case 'user':
      return `User (${join(getMemoryBaseDir(), 'agent-memory')}/)`
    case 'project':
      return 'Project (.claude/agent-memory/)'
    case 'local':
      return `Local (${getLocalAgentMemoryDir('...')})`
    default:
      return 'None'
  }
}

/**
 * Load persistent memory for an agent with memory enabled.
 * Creates the memory directory if needed and returns a prompt with memory contents.
 *
 * @param agentType The agent's type name (used as directory name)
 * @param scope 'user' for ~/.claude/agent-memory/ or 'project' for .claude/agent-memory/
 */
export function loadAgentMemoryPrompt(
  agentType: string,
  scope: AgentMemoryScope,
): string {
  let scopeNote: string
  switch (scope) {
    case 'user':
      scopeNote =
        '- Since this memory is user-scope, keep learnings general since they apply across all projects'
      break
    case 'project':
      scopeNote =
        '- Since this memory is project-scope and shared with your team via version control, tailor your memories to this project'
      break
    case 'local':
      scopeNote =
        '- Since this memory is local-scope (not checked into version control), tailor your memories to this project and machine'
      break
  }

  const memoryDir = getAgentMemoryDir(agentType, scope)
  const driveDir = scope === 'user' ? driveAgentMemoryDir(agentType) : undefined
  if (driveDir) return loadDriveAgentMemoryPrompt(driveDir)

  // Fire-and-forget: this runs at agent-spawn time inside a sync
  // getSystemPrompt() callback (called from React render in AgentDetail.tsx,
  // so it cannot be async). The spawned agent won't try to Write until after
  // a full API round-trip, by which time mkdir will have completed. Even if
  // it hasn't, FileWriteTool does its own mkdir of the parent directory.
  void ensureMemoryDirExists(memoryDir)

  const coworkExtraGuidelines =
    process.env.GIZZI_COWORK_MEMORY_EXTRA_GUIDELINES
  return buildMemoryPrompt({
    displayName: 'Persistent Agent Memory',
    memoryDir,
    extraGuidelines:
      coworkExtraGuidelines && coworkExtraGuidelines.trim().length > 0
        ? [scopeNote, coworkExtraGuidelines]
        : [scopeNote],
  })
}

/**
 * Prompt for user-scope agent memory on the Memory Drive: one topic file of
 * one-line memories. gizzi adds source/date/id, commits and syncs at the end
 * of the turn (same path as the main auto-memory checkout).
 */
function loadDriveAgentMemoryPrompt(driveDir: string): string {
  const file = join(driveDir, 'memory.md')
  let current = ''
  try {
    current = readFileSync(file, 'utf8').split('\n').slice(0, 200).join('\n')
  } catch {
    // Not created yet.
  }
  return [
    '# Persistent Agent Memory',
    '',
    `Your memory is the file \`${file}\` in the user's Memory Drive. It is shared across projects, so keep learnings general.`,
    'To save something, add ONE line `- <fact>` to that file with your file write/edit tools (create it with a `# <agent> memory` heading if it is missing). To change a memory, edit its text and keep the `[...]` metadata; to forget one, delete its line.',
    'gizzi adds the source, date and id, commits and syncs at the end of the turn. No frontmatter, one fact per line, never credentials, transcripts or raw logs.',
    '',
    current.trim() ? `## Current memory\n\n${current.trim()}` : '## Current memory\n\n(empty)',
  ].join('\n')
}
