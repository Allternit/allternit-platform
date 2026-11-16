import { relative } from 'path'
import type { ToolUseContext } from '../../Tool'
import { getCwd } from '../../utils/cwd'
import { cacheKeys } from '../../utils/fileStateCache'

/**
 * `/context files`: the files read into this session's context. This was
 * `/files` until the Files drive took that name (SPEC section 18, Eoj
 * 2026-11-15); the behavior is unchanged.
 */
export function filesInContext(context: Pick<ToolUseContext, 'readFileState'>): string {
  const files = context.readFileState ? cacheKeys(context.readFileState) : []
  if (files.length === 0) return 'No files in context'
  const fileList = files.map(file => relative(getCwd(), file)).join('\n')
  return `Files in context:\n${fileList}`
}

/** `/context files` (also `/context f`). */
export function wantsFilesInContext(args: string | undefined): boolean {
  const a = (args ?? '').trim().toLowerCase()
  return a === 'files' || a === 'f'
}
