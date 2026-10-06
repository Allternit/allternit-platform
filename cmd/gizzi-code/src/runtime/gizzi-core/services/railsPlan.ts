/**
 * Allternit Factory plan publishing for gizzi-code.
 *
 * When the user approves a plan in plan mode, the approved plan markdown is
 * parsed into a flat todo list and published to the Factory WIH DAG via
 * POST /api/factory/plan/from-text. The DAG then backs the Factory todo
 * panel in the TUI (see RailsTaskList.tsx).
 *
 * Everything here is best-effort: publishing must never throw into the
 * ExitPlanMode flow, so failures are logged and reported as `null`.
 */

import { logForDiagnosticsNoPII } from 'src/shared/utils/diagLogs.js'
import { errorMessage } from 'src/shared/utils/errors.js'
import {
  apiFetchJson,
  getAllternitApiConfig,
} from '../../services/api/allternitApi.js'

export type RailsPlanTodo = {
  title: string
  depth: number
  done: boolean
}

export type RailsPlanPublishResult = {
  dag_id: string
  root_node_id: string
  node_count: number
}

/**
 * Publish a parsed plan to the Factory DAG. Returns the created DAG
 * identity on success, null on any failure (logged, never thrown).
 */
export async function publishPlanToRails(
  title: string,
  todos: RailsPlanTodo[],
): Promise<RailsPlanPublishResult | null> {
  try {
    const config = getAllternitApiConfig()
    const result = await apiFetchJson<RailsPlanPublishResult>(
      config,
      '/api/factory/plan/from-text',
      {
        method: 'POST',
        body: JSON.stringify({ title, todos }),
      },
    )
    if (
      !result ||
      typeof result.dag_id !== 'string' ||
      typeof result.node_count !== 'number'
    ) {
      throw new Error('unexpected plan/from-text response shape')
    }
    logForDiagnosticsNoPII('info', 'rails_plan_published', {
      dag_id: result.dag_id,
      node_count: result.node_count,
    })
    return result
  } catch (error) {
    logForDiagnosticsNoPII('info', 'rails_plan_publish_failed', {
      error: errorMessage(error),
    })
    return null
  }
}

/** Strip inline markdown emphasis/code markers from a heading or todo title. */
function stripMarkdownInline(text: string): string {
  return text.replace(/[*_`~]/g, '').trim()
}

const CODE_FENCE_RE = /^\s*```/
const HEADING_RE = /^#{1,2}\s+(.+)$/
// Bullet (possibly a checkbox) with leading indentation preserved in group 1.
const BULLET_RE = /^(\s*)[-*+]\s+(?:\[( |x|X)\]\s+)?(.*)$/

/**
 * Parse approved plan markdown into the flat todo shape the DAG endpoint
 * expects:
 * - title: first `# ` / `## ` heading (markdown stripped); falls back to the
 *   first non-empty line, then to the literal "plan".
 * - todos: every `- [ ]` / `- [x]` / `* [ ]` checkbox line and every plain
 *   `- foo` bullet (done=false). depth = 0 for top-level bullets, +1 per
 *   2-space indent step, capped at 3 (tabs count as 2 spaces).
 * - everything else (other headings, prose, code fences) is ignored.
 */
export function parsePlanTodos(planMarkdown: string): {
  title: string
  todos: RailsPlanTodo[]
} {
  let title: string | null = null
  let firstNonEmptyLine: string | null = null
  let inCodeFence = false
  const todos: RailsPlanTodo[] = []

  for (const rawLine of planMarkdown.split(/\r?\n/)) {
    if (CODE_FENCE_RE.test(rawLine)) {
      inCodeFence = !inCodeFence
      continue
    }
    if (inCodeFence) continue

    const trimmed = rawLine.trim()
    if (!trimmed) continue
    if (firstNonEmptyLine === null) firstNonEmptyLine = trimmed

    if (title === null) {
      const heading = HEADING_RE.exec(trimmed)
      if (heading) {
        title = stripMarkdownInline(heading[1])
        continue
      }
    }

    const bullet = BULLET_RE.exec(rawLine)
    if (!bullet) continue
    const indent = bullet[1].replace(/\t/g, '  ').length
    const depth = Math.min(3, Math.floor(indent / 2))
    const done = (bullet[2] ?? '').toLowerCase() === 'x'
    const todoTitle = stripMarkdownInline(bullet[3])
    if (!todoTitle) continue
    todos.push({ title: todoTitle, depth, done })
  }

  return {
    title: title ?? (firstNonEmptyLine ?? 'plan'),
    todos,
  }
}
