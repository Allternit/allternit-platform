// Same name and description as the runtime builtin (runtime/tools/builtins/automation.ts),
// plus one paragraph on what "this thread" means in a terminal.
import { feature } from 'bun:bundle'
import AUTOMATION_TXT from '@/runtime/tools/builtins/automation.txt'
import { getFeatureValue_CACHED_WITH_REFRESH } from '../../services/analytics/growthbook.js'
import { DEFAULT_CRON_JITTER_CONFIG } from '../../utils/cronTasks.js'
import { isEnvTruthy } from '../../utils/envUtils.js'

export const AUTOMATION_TOOL_NAME = 'automation'

export const DESCRIPTION = "Create, list, change, delete or run the user's scheduled tasks"

/** Repeating tasks in the terminal session's queue stop after this many days (cronScheduler). */
export const SESSION_MAX_AGE_DAYS = DEFAULT_CRON_JITTER_CONFIG.recurringMaxAgeMs / (24 * 60 * 60 * 1000)

const TERMINAL_NOTE = `In this terminal: a message task with target this_thread goes into this Gizzi session's own queue. It runs here, between turns, only while this session stays open, and it ends with the session. Tell the user that when you create one. For anything that must outlast this session, or must run while the terminal is closed, use target new_session (it goes to the user's Allternit account, or to this computer's scheduler when there is no account). A heartbeat always goes there too. Older tasks from .claude/scheduled_tasks.json still run and show up in action list; you can delete them, but not change them.`

export const PROMPT: string = `${AUTOMATION_TXT.trimEnd()}\n\n${TERMINAL_NOTE}`

const SESSION_QUEUE_REFRESH_MS = 5 * 60 * 1000

/**
 * Gate for this terminal session's in-memory queue (and the scheduler that
 * fires it and the legacy .claude/scheduled_tasks.json entries): the
 * build-time `feature('AGENT_TRIGGERS')` flag plus the `tengu_kairos_cron`
 * gate (default true, src/constants/gizziGates.ts). `GIZZI_CODE_DISABLE_CRON`
 * turns it off locally.
 *
 * Called lazily (tool calls, useEffect, tips), never at module scope, so the
 * gate cache has had a chance to load.
 */
export function isKairosCronEnabled(): boolean {
  return feature('AGENT_TRIGGERS')
    ? !isEnvTruthy(process.env.GIZZI_CODE_DISABLE_CRON) &&
        getFeatureValue_CACHED_WITH_REFRESH('tengu_kairos_cron', true, SESSION_QUEUE_REFRESH_MS)
    : false
}
