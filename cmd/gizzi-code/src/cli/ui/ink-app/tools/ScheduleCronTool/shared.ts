import { getTeammateContext } from '../../utils/teammateContext.js'

/** Upper bound on scheduled jobs (durable + session) per project. */
export const MAX_CRON_JOBS = 50

/**
 * Cron jobs created by an in-process teammate are tagged with its agentId so
 * the scheduler routes fires to that teammate's queue (useScheduledTasks).
 * Teammate crons are always session-only.
 */
export function getCronOwnerAgentId(): string | undefined {
  return getTeammateContext()?.agentId
}
