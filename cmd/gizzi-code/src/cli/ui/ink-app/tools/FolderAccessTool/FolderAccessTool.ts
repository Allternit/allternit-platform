/**
 * folder_access in the Gizzi TUI: the Ink adapter for the runtime builtin of the
 * same name (contract: {action: request|request_delete, paths[1..5], reason} →
 * {granted: [resolved]}; always asks).
 *
 * Both actions add the folders to the session's working directories (the same
 * grant /add-dir makes for the session), so Ink's file and shell permission
 * checks treat them as granted, and record them in `folderGrants.ts`.
 * `request_delete` also allows real deletes; without it, Ink's permission path
 * moves deletes there to `<folder>/_to_delete/` (deleteRedirect.ts), the same
 * rule as the runtime's FolderGrants.
 *
 * Always asks: `requiresUserInteraction()` + an `ask` result is bypass-immune
 * in the Ink permission path (step 1e), so no mode and no saved "always" rule
 * skips the question. With nobody to ask (dontAsk, async agents) it is refused.
 */
import { realpath, stat } from 'fs/promises'
import { homedir } from 'os'
import { parse, resolve } from 'path'
import { z } from 'zod/v4'
import {
  getAdditionalDirectoriesForGizziMd,
  getOriginalCwd,
  setAdditionalDirectoriesForClaudeMd,
} from '../../bootstrap/state.js'
import { buildTool, type ToolDef } from '../../Tool.js'
import { lazySchema } from '../../utils/lazySchema.js'
import { expandPath } from '../../utils/path.js'
import { pathInWorkingPath } from '../../utils/permissions/pathInWorkingPath.js'
import { applyPermissionUpdate } from '../../utils/permissions/PermissionUpdate.js'
import { SandboxManager } from '../../utils/sandbox/sandbox-adapter.js'
import { accessGrantedFolders, deleteGrantedFolders } from './folderGrants.js'
import { DESCRIPTION, FOLDER_ACCESS_TOOL_NAME, PROMPT } from './prompt.js'
import { renderToolResultMessage, renderToolUseMessage } from './UI.js'

const inputSchema = lazySchema(() =>
  z.strictObject({
    action: z
      .enum(['request', 'request_delete'])
      .describe(
        '"request": read and write in the folders (a delete moves the file to <folder>/_to_delete/); "request_delete": also allow real deletes',
      ),
    paths: z
      .array(z.string().min(1))
      .min(1)
      .max(5)
      .describe('1 to 5 existing folders (absolute, ~/..., or relative to the project)'),
    reason: z
      .string()
      .min(1)
      .max(200)
      .describe('What you will do there, in plain words (the person sees it)'),
  }),
)
type InputSchema = ReturnType<typeof inputSchema>

const outputSchema = lazySchema(() =>
  z.object({
    action: z.enum(['request', 'request_delete']),
    granted: z.array(z.string()),
  }),
)
type OutputSchema = ReturnType<typeof outputSchema>
export type Output = z.infer<OutputSchema>

/**
 * Resolve one requested path to a real folder, with the runtime's rules: an
 * existing folder (relative paths are against the project), not the home
 * folder or the filesystem root, not inside the project.
 */
async function resolveFolder(raw: string): Promise<string> {
  const abs = resolve(getOriginalCwd(), expandPath(raw, getOriginalCwd()))
  let real: string
  try {
    real = await realpath(abs)
    if (!(await stat(real)).isDirectory()) throw new Error('not a folder')
  } catch {
    throw new Error(`Not an existing folder: ${raw}`)
  }
  if (real === parse(real).root) throw new Error(`The filesystem root can't be granted: ${raw}`)
  if (real === (await realpath(homedir()).catch(() => homedir()))) {
    throw new Error(`The home folder can't be granted; name a narrower folder: ${raw}`)
  }
  const project = await realpath(getOriginalCwd()).catch(() => getOriginalCwd())
  if (pathInWorkingPath(real, project)) {
    throw new Error(`${raw} is inside the project and needs no access`)
  }
  return real
}

async function resolveAll(paths: string[]): Promise<string[]> {
  const out: string[] = []
  for (const p of paths) {
    const folder = await resolveFolder(p)
    if (!out.includes(folder)) out.push(folder)
  }
  return out
}

export const FolderAccessTool = buildTool({
  name: FOLDER_ACCESS_TOOL_NAME,
  searchHint: 'ask the user for access to a folder outside the project',
  maxResultSizeChars: 10_000,
  async description() {
    return DESCRIPTION
  },
  async prompt() {
    return PROMPT
  },
  get inputSchema(): InputSchema {
    return inputSchema()
  },
  get outputSchema(): OutputSchema {
    return outputSchema()
  },
  userFacingName() {
    return 'Folder access'
  },
  shouldDefer: true,
  isConcurrencySafe() {
    return false
  },
  isReadOnly() {
    return false
  },
  requiresUserInteraction() {
    return true
  },
  toAutoClassifierInput(input) {
    return `${input.action} ${input.paths.join(' ')}`
  },
  async validateInput(input) {
    try {
      await resolveAll(input.paths)
    } catch (e) {
      return { result: false, message: (e as Error).message, errorCode: 1 }
    }
    return { result: true }
  },
  async checkPermissions(input) {
    const folders = await resolveAll(input.paths).catch(() => input.paths)
    const what =
      input.action === 'request_delete'
        ? 'delete files (not just move them to _to_delete/) in'
        : 'use'
    return {
      behavior: 'ask',
      message: `Allow the agent to ${what} ${folders.join(', ')}? ${input.reason}`,
      decisionReason: {
        type: 'safetyCheck',
        reason: `folder_access ${input.action}: ${input.reason}`,
        classifierApprovable: false,
      },
    }
  },
  renderToolUseMessage,
  renderToolResultMessage,
  async call(input, context) {
    const folders = await resolveAll(input.paths)

    const update = {
      type: 'addDirectories' as const,
      directories: folders,
      destination: 'session' as const,
    }
    const updated = applyPermissionUpdate(
      context.getAppState().toolPermissionContext,
      update,
    )
    context.setAppState(prev => ({ ...prev, toolPermissionContext: updated }))
    for (const f of folders) accessGrantedFolders.add(f)
    if (input.action === 'request_delete') {
      for (const f of folders) deleteGrantedFolders.add(f)
    }

    // Same follow-through as /add-dir: GIZZI.md lookup and the Bash sandbox.
    const current = getAdditionalDirectoriesForGizziMd()
    const added = folders.filter(f => !current.includes(f))
    if (added.length) setAdditionalDirectoriesForClaudeMd([...current, ...added])
    SandboxManager.refreshConfig()

    return { data: { action: input.action, granted: folders } }
  },
  mapToolResultToToolResultBlockParam(output, toolUseID) {
    return {
      type: 'tool_result',
      content: JSON.stringify({ granted: output.granted }),
      tool_use_id: toolUseID,
    }
  },
} satisfies ToolDef<InputSchema, Output>)
