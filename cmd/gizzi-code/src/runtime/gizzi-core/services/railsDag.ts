/**
 * Allternit Factory DAG poller for gizzi-code.
 *
 * Mirrors the user's Factory WIH DAGs (GET /api/factory/plans/dags?view=mine)
 * into the TUI so the Factory todo panel can render live node statuses. Same
 * shape as the inbox poller in railsPeer.ts: HTTP polling, dedupe by content
 * fingerprint, errors swallowed, gated on Factory peer mode.
 */

import { spawn } from 'node:child_process'
import { locateEngine } from 'src/cli/factory/engine.js'
import { logForDiagnosticsNoPII } from 'src/shared/utils/diagLogs.js'
import { errorMessage } from 'src/shared/utils/errors.js'
import {
  apiFetch,
  apiFetchJson,
  getAllternitApiConfig,
} from '../../services/api/allternitApi.js'

export type DagNodeStatus = 'NEW' | 'READY' | 'RUNNING' | 'DONE' | 'FAILED'

export type DagNodeDto = {
  node_id: string
  parent_node_id: string | null
  title: string
  status: DagNodeStatus
  ready: boolean
  assignee: string | null
  current_wih_id: string | null
}

export type DagViewItemDto = {
  dag_id: string
  root_title: string
  nodes: DagNodeDto[]
  ready_count: number
  done_count: number
}

export type ActiveWihDto = {
  wih_id: string
  dag_id: string
  node_id: string
  agent_id: string
  status: string
}

export type DagViewDto = {
  dags: DagViewItemDto[]
  active_wihs: ActiveWihDto[]
}

const RAILS_DAG_POLL_INTERVAL_MS = 3_000

/**
 * True when this session registered as a Factory peer (peer name exported by
 * railsPeer.ts registerRailsPeer). All Factory DAG behavior is gated on this
 * so non-peer sessions are completely unaffected.
 */
export function isRailsPeerMode(): boolean {
  return Boolean(process.env.ALLTERNIT_FACTORY_PEER_NAME)
}

/** The Factory peer name = our agent id for WIH pickup/close ownership. */
export function railsPeerAgentId(): string | null {
  return process.env.ALLTERNIT_FACTORY_PEER_NAME || null
}

// ─── WIH write-back ─────────────────────────────────────────────────────────
//
// pickup/close POSTs against /api/factory/wihs/*. Never throw: failures
// (network, 400/403/409 with the server's error message) come back as
// {ok:false, error} for inline display in the todo panel.

export type WihMutationResult = {
  ok: boolean
  /** Present on a successful pickup: the WIH the server created for us. */
  wih_id?: string
  error?: string
}

async function postWihMutation(
  path: string,
  body: Record<string, unknown>,
): Promise<WihMutationResult> {
  try {
    const config = getAllternitApiConfig()
    const res = await apiFetch(config, path, {
      method: 'POST',
      body: JSON.stringify(body),
    })
    if (!res.ok) {
      const text = await res.text().catch(() => '')
      let message = `HTTP ${res.status}`
      try {
        const parsed = JSON.parse(text) as { error?: string; message?: string }
        message = parsed.error ?? parsed.message ?? message
      } catch {
        // Non-JSON body — keep the status fallback.
      }
      logForDiagnosticsNoPII('info', 'rails_wih_mutation_failed', {
        path,
        status: res.status,
      })
      return { ok: false, error: `${message} (${res.status})` }
    }
    // Success bodies may carry fields we want (pickup returns wih_id);
    // parse best-effort — an empty/non-JSON body is fine.
    let wihId: string | undefined
    try {
      const text = await res.text().catch(() => '')
      const parsed = JSON.parse(text) as { wih_id?: unknown }
      if (typeof parsed.wih_id === 'string') wihId = parsed.wih_id
    } catch {
      // No JSON body — nothing to extract.
    }
    return { ok: true, wih_id: wihId }
  } catch (error) {
    logForDiagnosticsNoPII('info', 'rails_wih_mutation_failed', {
      path,
      error: errorMessage(error),
    })
    return { ok: false, error: errorMessage(error) }
  }
}

/**
 * Best-effort, fire-and-forget metadata stamp: report the picked-up WIH id
 * (plus dag/node ids) onto the Factory pane via
 * `allternit-factory pane pane report-metadata` so the pane's provenance
 * ledger can tie a pane to a WIH. Only runs when the engine exported
 * ALLTERNIT_FACTORY_PANE_ID. A stamp failure (missing engine binary, spawn
 * error, non-zero exit) must NEVER break or delay pickup —
 * everything here is untracked and swallowed to a debug log.
 */
function paneIdOrNull(): string | null {
  return process.env.ALLTERNIT_FACTORY_PANE_ID || null
}

function spawnPaneMetadata(
  args: string[],
  context?: Record<string, unknown>,
): void {
  const paneId = paneIdOrNull()
  if (!paneId) return
  try {
    const engine = locateEngine()
    if (!engine) {
      logForDiagnosticsNoPII('debug', 'rails_pane_metadata_failed', {
        error: 'Factory engine binary not found',
        ...context,
      })
      return
    }
    const child = spawn(engine.path, ['pane', 'pane', 'report-metadata', paneId, ...args], {
      detached: true,
      stdio: 'ignore',
      shell: false,
    })
    child.on('error', error => {
      // Spawn failure (e.g. the engine binary vanished) — log and skip;
      // the mutation itself already succeeded.
      logForDiagnosticsNoPII('debug', 'rails_pane_metadata_failed', {
        error: errorMessage(error),
        ...context,
      })
    })
    child.unref()
  } catch (error) {
    logForDiagnosticsNoPII('debug', 'rails_pane_metadata_failed', {
      error: errorMessage(error),
      ...context,
    })
  }
}

function stampPickupOnPane(dagId: string, nodeId: string, wihId: string): void {
  spawnPaneMetadata(
    [
      '--token',
      `wihId=${wihId}`,
      '--token',
      `dagId=${dagId}`,
      '--token',
      `nodeId=${nodeId}`,
    ],
    { dagId, nodeId, wihId },
  )
}

/**
 * Counterpart to the pickup stamp: when the WIH closes (DONE or FAILED),
 * the pane's provenance tokens come off again so the pane ledger doesn't
 * keep pointing at finished work. Same never-breaks-the-mutation contract
 * as the stamp.
 */
function clearPaneTokensForClose(dagId: string, nodeId: string): void {
  spawnPaneMetadata(
    ['--clear-token', 'wihId', '--clear-token', 'dagId', '--clear-token', 'nodeId'],
    { dagId, nodeId },
  )
}

/**
 * Pick up a READY node: creates a WIH owned by this peer. 409 when the node
 * is not pickup-able (already taken, not ready); error message is surfaced
 * verbatim for inline panel display. On success the returned wih_id (when the
 * pane env carries ALLTERNIT_FACTORY_PANE_ID) is stamped onto the Factory pane.
 */
export function pickupWih(
  dagId: string,
  nodeId: string,
): Promise<WihMutationResult> {
  const agentId = railsPeerAgentId()
  if (!agentId) {
    return Promise.resolve({ ok: false, error: 'Factory peer mode is off' })
  }
  return postWihMutation('/api/factory/wihs/pickup', {
    dag_id: dagId,
    node_id: nodeId,
    agent_id: agentId,
  }).then(result => {
    if (result.ok && result.wih_id) {
      stampPickupOnPane(dagId, nodeId, result.wih_id)
    }
    return result
  })
}

/**
 * Close a WIH we own (status DONE or FAILED, auto-evidence v1). 403 when
 * the WIH is owned by a different agent; 400 when evidence is empty or the
 * status is outside DONE|FAILED (the server normalizes case). dagId/nodeId
 * are optional context threaded from the caller; on a successful close they
 * trigger the fire-and-forget pane token clear (no-op when the pane env is
 * absent).
 */
export function closeWih(
  wihId: string,
  evidence: string[],
  status: 'DONE' | 'FAILED' = 'DONE',
  dagId?: string,
  nodeId?: string,
): Promise<WihMutationResult> {
  const agentId = railsPeerAgentId()
  if (!agentId) {
    return Promise.resolve({ ok: false, error: 'Factory peer mode is off' })
  }
  return postWihMutation(
    `/api/factory/wihs/${encodeURIComponent(wihId)}/close`,
    { status, evidence, agent_id: agentId },
  ).then(result => {
    if (result.ok && dagId && nodeId) {
      clearPaneTokensForClose(dagId, nodeId)
    }
    return result
  })
}

// ─── DAG node mutations ─────────────────────────────────────────────────────
//
// rename/reparent/delete against /api/factory/plan/:dag_id/nodes/:node_id.
// Same contract as the WIH mutations: never throw, 400/409 bodies carry an
// `error` string that is surfaced verbatim in the todo panel.

export type DagNodeMutationResult = { ok: boolean; error?: string }

async function dagNodeMutation(
  method: 'PATCH' | 'DELETE',
  dagId: string,
  nodeId: string,
  body?: Record<string, unknown>,
): Promise<DagNodeMutationResult> {
  const path = `/api/factory/plan/${encodeURIComponent(dagId)}/nodes/${encodeURIComponent(nodeId)}`
  try {
    const config = getAllternitApiConfig()
    const res = await apiFetch(config, path, {
      method,
      ...(body ? { body: JSON.stringify(body) } : {}),
    })
    if (!res.ok) {
      const text = await res.text().catch(() => '')
      let message = `HTTP ${res.status}`
      try {
        const parsed = JSON.parse(text) as { error?: string; message?: string }
        message = parsed.error ?? parsed.message ?? message
      } catch {
        // Non-JSON body — keep the status fallback.
      }
      logForDiagnosticsNoPII('info', 'rails_dag_node_mutation_failed', {
        path,
        method,
        status: res.status,
      })
      return { ok: false, error: `${message} (${res.status})` }
    }
    return { ok: true }
  } catch (error) {
    logForDiagnosticsNoPII('info', 'rails_dag_node_mutation_failed', {
      path,
      method,
      error: errorMessage(error),
    })
    return { ok: false, error: errorMessage(error) }
  }
}

/**
 * Rename a node (todo panel `e` key). 400 on an empty/invalid title; the
 * server's error string is surfaced inline.
 */
export function renameNode(
  dagId: string,
  nodeId: string,
  title: string,
): Promise<DagNodeMutationResult> {
  return dagNodeMutation('PATCH', dagId, nodeId, { title })
}

/**
 * Move a node under a new parent (`parent_node_id`, null = dag root). 409
 * when the move would create a cycle.
 */
export function reparentNode(
  dagId: string,
  nodeId: string,
  parentNodeId: string | null,
): Promise<DagNodeMutationResult> {
  return dagNodeMutation('PATCH', dagId, nodeId, { parent_node_id: parentNodeId })
}

/**
 * Delete a node (todo panel `D` key, y-confirm). 204 on success; 409 when
 * the node still has open work (body carries an `error` string).
 */
export function deleteNode(
  dagId: string,
  nodeId: string,
): Promise<DagNodeMutationResult> {
  return dagNodeMutation('DELETE', dagId, nodeId)
}

// ─── Immediate refresh ──────────────────────────────────────────────────────
//
// The poller dedupes by JSON fingerprint, so after a successful mutation a
// forced re-invocation of its poll cycle picks up the new state immediately
// instead of waiting for the next 3s tick. Only one listener runs per
// process; the ref is cleared when it stops.

let activeForcedPoll: (() => Promise<void>) | null = null

/**
 * Fire an immediate refetch of the DAG view. Fire-and-forget; poll errors
 * are already swallowed downstream. No-op when no listener is running.
 */
export function refreshRailsDagNow(): void {
  const poll = activeForcedPoll
  if (!poll) return
  poll().catch(() => {
    // Same swallow semantics as the interval poll.
  })
}

/**
 * Start polling the DAG view. Calls onUpdate only when the dags/active_wihs
 * content actually changed (JSON fingerprint). Errors are logged and
 * swallowed. Returns a stop function; safe to call when peer mode is off
 * (returns a no-op stop and never polls).
 */
export function startRailsDagListener(
  onUpdate: (dto: DagViewDto) => void,
): () => void {
  if (!isRailsPeerMode()) {
    return () => {}
  }

  const config = getAllternitApiConfig()
  let lastFingerprint: string | null = null
  let pollIntervalId: ReturnType<typeof setInterval> | null = null

  async function pollOnce(): Promise<void> {
    try {
      const dto = await apiFetchJson<DagViewDto>(
        config,
        '/api/factory/plans/dags?view=mine',
      )
      const fingerprint = JSON.stringify([dto.dags, dto.active_wihs])
      if (fingerprint === lastFingerprint) return
      lastFingerprint = fingerprint
      onUpdate(dto)
    } catch (error) {
      logForDiagnosticsNoPII('info', 'rails_dag_poll_error', {
        error: errorMessage(error),
      })
    }
  }

  activeForcedPoll = pollOnce

  // Poll immediately, then every 3 seconds.
  pollOnce().catch(() => {
    // Next interval retries; DAG polling must never crash the host process.
  })
  pollIntervalId = setInterval(() => {
    pollOnce().catch(() => {
      // Next interval retries; DAG polling must never crash the host process.
    })
  }, RAILS_DAG_POLL_INTERVAL_MS)

  function stop(): void {
    if (activeForcedPoll === pollOnce) {
      activeForcedPoll = null
    }
    if (pollIntervalId) {
      clearInterval(pollIntervalId)
      pollIntervalId = null
    }
  }

  // Stop polling when the process exits (the bridge also stops on unmount).
  process.once('exit', stop)

  return stop
}
