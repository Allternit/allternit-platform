// @ts-nocheck
/**
 * folder_access in the Gizzi TUI: the Ink adapter for the runtime builtin of the
 * same name (contract: {action: request|request_delete, paths[1..5], reason} →
 * {granted: [resolved]}; always asks).
 *
 * A `request` grant adds the folders to the session's working directories (the
 * same grant /add-dir makes for the session), so Ink's file and shell
 * permission checks treat them as granted. A `request_delete` grant is recorded
 * per session in `deleteGrantedFolders` for the delete path to consult.
 *
 * Always asks: `requiresUserInteraction()` + an `ask` result is bypass-immune
 * in the Ink permission path (step 1e), so no mode and no saved "always" rule
 * skips the question. With nobody to ask (dontAsk, async agents) it is refused.
 */
import { stat } from 'fs/promises'
import { dirname, resolve } from 'path'
import { z } from 'zod/v4'
import {
  getAdditionalDirectoriesForGizziMd,
  setAdditionalDirectoriesForClaudeMd,
} from '../../bootstrap/state.js'
import { buildTool, type ToolDef } from '../../Tool.js'
import { lazySchema } from '../../utils/lazySchema.js'
import { expandPath } from '../../utils/path.js'
import { allWorkingDirectories } from '../../utils/permissions/filesystem.js'
import { pathInWorkingPath } from '../../utils/permissions/pathInWorkingPath.js'
import { applyPermissionUpdate } from '../../utils/permissions/PermissionUpdate.js'
import { SandboxManager } from '../../utils/sandbox/sandbox-adapter.js'
import { DESCRIPTION, FOLDER_ACCESS_TOOL_NAME, PROMPT } from './prompt.js'
import { renderToolResultMessage, renderToolUseMessage } from './UI.js'

const inputSchema = lazySchema(() =>
  z.strictObject({
    action: z
      .enum(['request', 'request_delete'])
      .describe(
        '"request" grants access to the folders for this session; "request_delete" lets deletes inside granted folders remove files instead of moving them to _to_delete/',
      ),
    paths: z
      .array(z.string().min(1))
      .min(1)
      .max(5)
      .describe('1 to 5 folder paths (absolute, relative, or starting with ~)'),
    reason: z
      .string()
      .min(1)
      .max(200)
      .describe('One short sentence the user sees: why you need this access'),
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

/** Folders the user approved `request_delete` for, this process (session). */
export const deleteGrantedFolders = new Set<string>()

/** Whether a real delete (not the _to_delete/ move) is granted for this path. */
export function isDeleteGranted(target: string): boolean {
  const abs = resolve(expandPath(target))
  for (const folder of deleteGrantedFolders) {
    if (pathInWorkingPath(abs, folder)) return true
  }
  return false
}

/** Resolve one requested path to the folder it names (a file names its folder). */
async function resolveFolder(raw: string): Promise<string> {
  const abs = resolve(expandPath(raw))
  let info
  try {
    info = await stat(abs)
  } catch {
    throw new Error(`Folder not found: ${raw}`)
  }
  return info.isDirectory() ? abs : dirname(abs)
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

    if (input.action === 'request_delete') {
      const working = allWorkingDirectories(
        context.getAppState().toolPermissionContext,
      )
      const ungranted = folders.filter(
        f => !Array.from(working).some(w => pathInWorkingPath(f, w)),
      )
      if (ungranted.length) {
        throw new Error(
          `Request access first (action "request") for: ${ungranted.join(', ')}`,
        )
      }
      for (const f of folders) deleteGrantedFolders.add(f)
      return { data: { action: input.action, granted: folders } }
    }

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
