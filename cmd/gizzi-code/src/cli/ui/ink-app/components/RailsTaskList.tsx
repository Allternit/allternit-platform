/**
 * Factory Task List
 *
 * Renders the Factory WIH DAGs mirrored into AppState (railsDag slice,
 * written by RailsDagBridge) using the same visual language as TaskListV2:
 * figures-style glyphs, 30s recent-completion TTL with a collapse summary
 * row, and the same terminal-height display budget. Self-hides when Factory
 * peer mode is off (no updatedAt) or there are no dags.
 *
 * Focused keys (peer mode only): j/k move the selection across all node
 * rows; the footer lists what the selected row supports — t picks up a
 * READY node, d/x closes owned RUNNING work as DONE/FAILED, e renames the
 * selected node inline, D (shift+d) deletes it after a y/n confirm, and r
 * reparents it under another node or the dag root (edit/delete/reparent
 * are offered on every status; the server guards delete/reparent conflicts
 * with 409s surfaced inline). Write failures surface on the row for 5s.
 */

import * as React from 'react'
import { Box, Text } from '@/ink.js'
import { useAppState } from '../state/AppState.js'
import type { AppState } from '../state/AppStateStore.js'
import { useTerminalSize } from '../hooks/useTerminalSize.js'
import { truncateToWidth } from '../../../../shared/utils/format.js'
import { useKeybindings } from '../keybindings/useKeybinding.js'
import {
  useIsModalOverlayActive,
  useRegisterOverlay,
} from '../context/overlayContext.js'
import { Input } from './Input.js'
import {
  closeWih,
  deleteNode,
  isRailsPeerMode,
  pickupWih,
  renameNode,
  railsPeerAgentId,
  refreshRailsDagNow,
  reparentNode,
} from '@/runtime/gizzi-core/services/railsDag'

// Same budget rule as TaskListV2: rows<=10 hides the panel entirely, else
// min(10, max(3, rows-14)) lines — dag headers count against it.
function maxDisplayForRows(rows: number): number {
  return rows <= 10 ? 0 : Math.min(10, Math.max(3, rows - 14))
}

const RECENT_DONE_TTL_MS = 30_000
const MAX_TIMEOUT_MS = 2_147_483_647

// Frontier-first status ordering: READY → RUNNING → FAILED → NEW → DONE.
// Unknown statuses (the server can emit non-canonical values, e.g. lowercase
// legacy 'done') fall back to the NEW rank so they still render.
const STATUS_RANK: Record<string, number> = {
  READY: 0,
  RUNNING: 1,
  FAILED: 2,
  NEW: 3,
  DONE: 4,
}
const UNKNOWN_STATUS_RANK = 3

function glyphFor(status: string): {
  icon: string
  color: 'success' | 'gizzi' | 'error' | undefined
  dim: boolean
} {
  switch (String(status).toUpperCase()) {
    case 'DONE':
      return { icon: '✔', color: 'success', dim: true }
    case 'RUNNING':
      return { icon: '◐', color: 'gizzi', dim: false }
    case 'READY':
      return { icon: '○', color: undefined, dim: false }
    case 'FAILED':
      return { icon: '✖', color: 'error', dim: false }
    case 'NEW':
    default:
      return { icon: '·', color: undefined, dim: true }
  }
}

/** Tree depth from parent_node_id chains, capped at 3. */
type RailsDag = AppState['railsDag']['dags'][number]
type RailsDagNode = RailsDag['nodes'][number]

/** Normalized (uppercase) node status — the server can emit legacy
 * lowercase values like 'done'. */
function statusOf(node: RailsDagNode): string {
  return String(node.status).toUpperCase()
}

function nodeDepths(dag: RailsDag): Map<string, number> {
  const byParent = new Map<string, string>() // node_id -> parent_node_id
  for (const node of dag.nodes) {
    if (node.parent_node_id) byParent.set(node.node_id, node.parent_node_id)
  }
  const depths = new Map<string, number>()
  for (const node of dag.nodes) {
    let depth = 0
    let cur: string | null = node.parent_node_id
    while (cur && depth < 3) {
      depth += 1
      cur = byParent.get(cur) ?? null
    }
    depths.set(node.node_id, depth)
  }
  return depths
}

type DagLine =
  | { kind: 'header'; key: string; dag: RailsDag }
  | { kind: 'node'; key: string; dagId: string; node: RailsDagNode; depth: number }
  | { kind: 'done-summary'; key: string; dagId: string; count: number }

// ─── Selection (pure logic, unit-tested in RailsTaskList.test.ts) ──────────

export type ActionableKind = 'take' | 'done'

/**
 * Which write-back action (if any) a node row supports for this peer:
 * READY → 'take' (pickup), RUNNING owned by us → 'done' (close).
 */
export function actionableKindFor(
  status: string,
  assignee: string | null,
  agentId: string | null,
): ActionableKind | null {
  const normalized = String(status).toUpperCase()
  if (normalized === 'READY') return 'take'
  if (normalized === 'RUNNING' && agentId && assignee === agentId) return 'done'
  return null
}

export type RowAction = 'take' | 'done' | 'fail' | 'edit' | 'delete' | 'reparent'

/**
 * Every action the panel offers on a node row for this peer. edit/delete/
 * reparent are available on ANY status — the server guards the dangerous
 * cases (delete with open children/active WIH 409s, reparent cycles 409),
 * so the panel only needs to offer them; take is READY-only, done/fail only
 * on RUNNING work owned by this peer. Status is normalized
 * case-insensitively, mirroring actionableKindFor.
 */
export function actionsFor(
  status: string,
  assignee: string | null,
  agentId: string | null,
): RowAction[] {
  const normalized = String(status).toUpperCase()
  const actions: RowAction[] = []
  if (normalized === 'READY') actions.push('take')
  if (normalized === 'RUNNING' && agentId && assignee === agentId) {
    actions.push('done', 'fail')
  }
  actions.push('edit', 'delete', 'reparent')
  return actions
}

/** Clamp a selection index into [0, count-1]; -1 when nothing is selectable. */
export function clampSelectionIndex(index: number, count: number): number {
  if (count <= 0) return -1
  return Math.max(0, Math.min(index, count - 1))
}

/**
 * Reparent targets for `nodeId`: every node in the same dag except the node
 * itself and its own descendants (walking parent_node_id child chains —
 * offering a descendant would create a cycle the server would 409 on).
 * Generic over the node shape so tests can pass minimal literals.
 */
export function reparentCandidates<T extends {
  node_id: string
  parent_node_id: string | null
}>(nodes: T[], nodeId: string): T[] {
  const childIdsByParent = new Map<string, string[]>()
  for (const node of nodes) {
    if (!node.parent_node_id) continue
    const siblings = childIdsByParent.get(node.parent_node_id)
    if (siblings) siblings.push(node.node_id)
    else childIdsByParent.set(node.parent_node_id, [node.node_id])
  }
  const excluded = new Set<string>([nodeId])
  // BFS down from the node itself: everything reachable via child links is
  // a descendant and must not be offered as a new parent.
  const queue = [nodeId]
  while (queue.length > 0) {
    const current = queue.shift()!
    for (const childId of childIdsByParent.get(current) ?? []) {
      if (excluded.has(childId)) continue
      excluded.add(childId)
      queue.push(childId)
    }
  }
  return nodes.filter(node => !excluded.has(node.node_id))
}

type SelectableRow = {
  key: string
  dagId: string
  actions: RowAction[]
  node: RailsDagNode
}

const ROW_ERROR_TTL_MS = 5_000

/** Auto-evidence line for a panel-initiated close (v1, no prompt input). */
export function closeEvidenceFor(
  status: 'DONE' | 'FAILED',
  agentId: string | null,
): string {
  const verb = status === 'FAILED' ? 'failed' : 'closed'
  return `${verb} from gizzi-code todo panel by ${agentId ?? 'unknown'}`
}

/**
 * Sub-modes entered from the focused panel. `edit` swaps the selected row
 * for an inline Input; `confirm-delete` and `reparent` keep the row and
 * attach a small prompt/candidate list beneath it. While a sub-mode is
 * open the main action keys (t/d/x/e/D/r) are suspended.
 */
type SubMode =
  | { kind: 'edit'; dagId: string; nodeId: string; key: string }
  | { kind: 'confirm-delete'; dagId: string; nodeId: string; key: string }
  | {
      kind: 'reparent'
      dagId: string
      nodeId: string
      key: string
      candidates: RailsDagNode[]
      /** 0 = the synthetic "(root)" entry; i+1 indexes into candidates. */
      index: number
    }

export function RailsTaskList(): React.ReactElement | null {
  const railsDag = useAppState(s => s.railsDag)
  const { rows, columns } = useTerminalSize()
  const [, forceUpdate] = React.useState(0)

  // Track when each node was last observed transitioning to DONE.
  const doneTimestampsRef = React.useRef(new Map<string, number>())
  const previousDoneIdsRef = React.useRef<Set<string> | null>(null)

  const dags = railsDag?.dags ?? []
  const currentDoneIds = new Set<string>()
  for (const dag of dags) {
    for (const node of dag.nodes) {
      if (statusOf(node) === 'DONE') currentDoneIds.add(`${dag.dag_id}:${node.node_id}`)
    }
  }
  if (previousDoneIdsRef.current === null) {
    // First render: treat everything already DONE as old (collapsed).
    previousDoneIdsRef.current = currentDoneIds
  }
  const now = Date.now()
  for (const id of currentDoneIds) {
    if (!previousDoneIdsRef.current.has(id)) {
      doneTimestampsRef.current.set(id, now)
    }
  }
  for (const id of doneTimestampsRef.current.keys()) {
    if (!currentDoneIds.has(id)) doneTimestampsRef.current.delete(id)
  }
  previousDoneIdsRef.current = currentDoneIds

  // Re-render when the next recent DONE expires (same TTL pattern as
  // TaskListV2).
  React.useEffect(() => {
    if (doneTimestampsRef.current.size === 0) return
    const currentNow = Date.now()
    let earliestExpiry = Infinity
    for (const ts of doneTimestampsRef.current.values()) {
      const expiry = ts + RECENT_DONE_TTL_MS
      if (expiry > currentNow && expiry < earliestExpiry) {
        earliestExpiry = expiry
      }
    }
    if (earliestExpiry === Infinity) return
    const delay = Math.max(0, Math.min(earliestExpiry - currentNow, MAX_TIMEOUT_MS))
    const timer = setTimeout(
      force => force((n: number) => n + 1),
      delay,
      forceUpdate,
    )
    return () => clearTimeout(timer)
  }, [dags])

  const maxDisplay = maxDisplayForRows(rows)

  // Build display lines: one header per dag, then nodes frontier-first with
  // children indented under parents; DONE nodes past the TTL collapse into a
  // single `✔ N done ▸` summary row per dag.
  const lines: DagLine[] = []
  for (const dag of dags) {
    lines.push({ kind: 'header', key: `h:${dag.dag_id}`, dag })
    const depths = nodeDepths(dag)
    const sorted = dag.nodes
      .map((node, index) => ({ node, index }))
      .sort((a, b) => {
        const rankDiff =
          (STATUS_RANK[statusOf(a.node)] ?? UNKNOWN_STATUS_RANK) -
          (STATUS_RANK[statusOf(b.node)] ?? UNKNOWN_STATUS_RANK)
        return rankDiff !== 0 ? rankDiff : a.index - b.index
      })
    let collapsedDone = 0
    for (const { node } of sorted) {
      if (statusOf(node) === 'DONE') {
        const ts = doneTimestampsRef.current.get(`${dag.dag_id}:${node.node_id}`)
        if (!ts || now - ts >= RECENT_DONE_TTL_MS) {
          collapsedDone += 1
          continue
        }
      }
      lines.push({
        kind: 'node',
        key: `n:${dag.dag_id}:${node.node_id}`,
        dagId: dag.dag_id,
        node,
        depth: depths.get(node.node_id) ?? 0,
      })
    }
    if (collapsedDone > 0) {
      lines.push({
        kind: 'done-summary',
        key: `s:${dag.dag_id}`,
        dagId: dag.dag_id,
        count: collapsedDone,
      })
    }
  }

  const visibleLines = lines.slice(0, maxDisplay)
  const hiddenCount = lines.length - visibleLines.length

  // ─── Interactivity (write-back) ──────────────────────────────────────────
  //
  // Focus model follows the established overlay pattern (overlayContext.tsx):
  // `tab` focuses the panel, which registers a modal overlay so PromptInput
  // releases the text input and its keybindings; `esc` blurs, unregistering
  // the overlay so keys return to the prompt. Keys only bind while focused
  // (useKeybindings isActive), so unfocused/non-rails behavior is untouched.

  const [focused, setFocused] = React.useState(false)
  const [selectionKey, setSelectionKey] = React.useState<string | null>(null)
  const [rowError, setRowError] = React.useState<{
    key: string
    message: string
  } | null>(null)
  const [subMode, setSubMode] = React.useState<SubMode | null>(null)
  const [editValue, setEditValue] = React.useState('')

  const agentId = railsPeerAgentId()
  // Every visible node row is selectable — take/done/fail stay gated on the
  // row's actions, but edit/delete/reparent are offered on any status.
  const selectable: SelectableRow[] = []
  if (isRailsPeerMode() && maxDisplay > 0) {
    for (const line of visibleLines) {
      if (line.kind !== 'node') continue
      selectable.push({
        key: line.key,
        dagId: line.dagId,
        actions: actionsFor(line.node.status, line.node.assignee, agentId),
        node: line.node,
      })
    }
  }
  const selectableCount = selectable.length
  const foundIndex = selectionKey
    ? selectable.findIndex(row => row.key === selectionKey)
    : -1
  const selectionIndex =
    foundIndex >= 0 ? foundIndex : clampSelectionIndex(0, selectableCount)

  const panelVisible = Boolean(
    railsDag &&
      railsDag.updatedAt !== null &&
      dags.length > 0 &&
      maxDisplay > 0 &&
      isRailsPeerMode(),
  )
  const isModalOverlayActive = useIsModalOverlayActive()

  const selectDelta = (delta: number): void => {
    const next = clampSelectionIndex(selectionIndex + delta, selectableCount)
    if (next >= 0) setSelectionKey(selectable[next]!.key)
  }

  const takeSelected = async (): Promise<void> => {
    const selected = selectable[selectionIndex]
    if (!selected || !selected.actions.includes('take')) return
    const result = await pickupWih(selected.dagId, selected.node.node_id)
    if (result.ok) {
      setRowError(null)
      refreshRailsDagNow()
    } else {
      setRowError({ key: selected.key, message: result.error ?? 'pickup failed' })
    }
  }

  const closeSelected = async (status: 'DONE' | 'FAILED'): Promise<void> => {
    const selected = selectable[selectionIndex]
    const action: RowAction = status === 'FAILED' ? 'fail' : 'done'
    if (
      !selected ||
      !selected.actions.includes(action) ||
      !selected.node.current_wih_id
    ) {
      return
    }
    const result = await closeWih(
      selected.node.current_wih_id,
      [closeEvidenceFor(status, railsPeerAgentId())],
      status,
      selected.dagId,
      selected.node.node_id,
    )
    if (result.ok) {
      setRowError(null)
      refreshRailsDagNow()
    } else {
      setRowError({ key: selected.key, message: result.error ?? 'close failed' })
    }
  }

  // ─── Sub-modes (edit / delete confirm / reparent picker) ─────────────────

  const startEdit = (): void => {
    const selected = selectable[selectionIndex]
    if (!selected || !selected.actions.includes('edit')) return
    setEditValue(selected.node.title)
    setSubMode({
      kind: 'edit',
      dagId: selected.dagId,
      nodeId: selected.node.node_id,
      key: selected.key,
    })
  }

  const submitEdit = async (): Promise<void> => {
    if (subMode?.kind !== 'edit') return
    const title = editValue.trim()
    const { dagId, nodeId, key } = subMode
    if (!title) {
      // Empty submission (e.g. after esc cleared the field) cancels.
      setSubMode(null)
      return
    }
    const result = await renameNode(dagId, nodeId, title)
    if (result.ok) {
      setSubMode(null)
      setRowError(null)
      refreshRailsDagNow()
    } else {
      setSubMode(null)
      setRowError({ key, message: result.error ?? 'rename failed' })
    }
  }

  const startConfirmDelete = (): void => {
    const selected = selectable[selectionIndex]
    if (!selected || !selected.actions.includes('delete')) return
    setSubMode({
      kind: 'confirm-delete',
      dagId: selected.dagId,
      nodeId: selected.node.node_id,
      key: selected.key,
    })
  }

  const confirmDelete = async (): Promise<void> => {
    if (subMode?.kind !== 'confirm-delete') return
    const { dagId, nodeId, key } = subMode
    const result = await deleteNode(dagId, nodeId)
    setSubMode(null)
    if (result.ok) {
      setRowError(null)
      refreshRailsDagNow()
    } else {
      setRowError({ key, message: result.error ?? 'delete failed' })
    }
  }

  const startReparent = (): void => {
    const selected = selectable[selectionIndex]
    if (!selected || !selected.actions.includes('reparent')) return
    const dag = dags.find(d => d.dag_id === selected.dagId)
    if (!dag) return
    setSubMode({
      kind: 'reparent',
      dagId: selected.dagId,
      nodeId: selected.node.node_id,
      key: selected.key,
      candidates: reparentCandidates(dag.nodes, selected.node.node_id),
      index: 0,
    })
  }

  const reparentDelta = (delta: number): void => {
    setSubMode(prev => {
      if (prev?.kind !== 'reparent') return prev
      // +1: the synthetic "(root)" entry occupies index 0.
      return {
        ...prev,
        index: clampSelectionIndex(prev.index + delta, prev.candidates.length + 1),
      }
    })
  }

  const confirmReparent = async (): Promise<void> => {
    if (subMode?.kind !== 'reparent') return
    const { dagId, nodeId, key, index, candidates } = subMode
    // Index 0 is the synthetic root entry → parent_node_id null.
    const parentNodeId = index === 0 ? null : (candidates[index - 1]?.node_id ?? null)
    const result = await reparentNode(dagId, nodeId, parentNodeId)
    setSubMode(null)
    if (result.ok) {
      setRowError(null)
      refreshRailsDagNow()
    } else {
      setRowError({ key, message: result.error ?? 'reparent failed' })
    }
  }

  // Focus grab: only while the panel is visible, unfocused, no modal
  // overlay (permission dialogs etc.) already owns the keys, and no sub-mode
  // is open.
  useKeybindings(
    {
      'railsDag:focus': () => {
        if (selectableCount === 0) return
        setFocused(true)
        setSelectionKey(prev => prev ?? selectable[0]?.key ?? null)
      },
    },
    {
      context: 'RailsDag',
      isActive: panelVisible && !focused && !isModalOverlayActive,
    },
  )

  // Focused keys: j/k/arrows move, t takes, d/x close done/failed, e/D/r
  // edit/delete/reparent, esc blurs. Suspended while a sub-mode is open.
  useKeybindings(
    {
      'select:next': () => selectDelta(1),
      'select:previous': () => selectDelta(-1),
      'railsDag:take': () => {
        void takeSelected()
      },
      'railsDag:done': () => {
        void closeSelected('DONE')
      },
      'railsDag:fail': () => {
        void closeSelected('FAILED')
      },
      'railsDag:edit': () => startEdit(),
      'railsDag:delete': () => startConfirmDelete(),
      'railsDag:reparent': () => startReparent(),
      'railsDag:blur': () => setFocused(false),
    },
    { context: 'RailsDag', isActive: focused && panelVisible && subMode === null },
  )

  // Sub-mode keys (delete confirm / reparent picker): j/k move the reparent
  // candidate cursor, enter/y confirms, n/esc cancels. The inline edit
  // sub-mode owns its keys through the Input component instead.
  useKeybindings(
    {
      'select:next': () => reparentDelta(1),
      'select:previous': () => reparentDelta(-1),
      'railsDag:confirm': () => {
        if (subMode?.kind === 'reparent') void confirmReparent()
        else if (subMode?.kind === 'confirm-delete') void confirmDelete()
      },
      'railsDag:cancel': () => setSubMode(null),
      'railsDag:blur': () => setSubMode(null),
    },
    {
      context: 'RailsDag',
      isActive:
        focused && panelVisible && subMode !== null && subMode.kind !== 'edit',
    },
  )

  // Modal overlay while focused: PromptInput gates its text input and
  // keybindings on useIsModalOverlayActive (PromptInput.tsx:2186), so this
  // is what actually "gives the panel the keys" and returns them on blur.
  // Kept registered through sub-modes so the prompt never steals keystrokes
  // from the inline Input or the y/n confirm.
  useRegisterOverlay('rails-dag-todo', focused && panelVisible)

  // Auto-blur when the selection pool empties (last visible node closed or
  // the dags view went quiet); drop any sub-mode with it.
  React.useEffect(() => {
    if (focused && selectableCount === 0) {
      setFocused(false)
      setSubMode(null)
    }
  }, [focused, selectableCount])

  // Drop a stale sub-mode when the panel hides (terminal resize, peer mode
  // off, dags emptied) so keys aren't stranded on an invisible overlay.
  React.useEffect(() => {
    if (!panelVisible && subMode) setSubMode(null)
  }, [panelVisible, subMode])

  // Row errors clear after a short TTL.
  React.useEffect(() => {
    if (!rowError) return
    const timer = setTimeout(() => setRowError(null), ROW_ERROR_TTL_MS)
    return () => clearTimeout(timer)
  }, [rowError])

  if (!railsDag || railsDag.updatedAt === null || dags.length === 0) {
    return null
  }

  if (maxDisplay === 0) return null

  const planPublish = railsDag.planPublish
  const maxTitleWidth = Math.max(15, columns - 20)
  const selectedKey = focused ? selectable[selectionIndex]?.key : null

  return (
    <Box flexDirection="column" marginTop={1} marginLeft={2}>
      {planPublish && (
        <Text dimColor>
          Plan tracked as dag {planPublish.dag_id} ({planPublish.node_count}{' '}
          nodes)
        </Text>
      )}
      {visibleLines.map(line => {
        if (line.kind === 'header') {
          return (
            <Text key={line.key} dimColor>
              <Text bold>{truncateToWidth(line.dag.root_title, maxTitleWidth)}</Text>
              {` — ${line.dag.ready_count} ready, ${line.dag.done_count} done`}
            </Text>
          )
        }
        if (line.kind === 'done-summary') {
          return (
            <Text key={line.key} dimColor>
              {`✔ ${line.count} done ▸`}
            </Text>
          )
        }
        const { icon, color, dim } = glyphFor(statusOf(line.node))
        const title = truncateToWidth(line.node.title, maxTitleWidth)
        const isSelected = line.key === selectedKey
        const error =
          rowError && rowError.key === line.key ? rowError.message : null

        // Inline edit: the selected row swaps to an Input pre-filled with
        // the current title. Enter renames; empty submission cancels.
        if (subMode?.kind === 'edit' && subMode.key === line.key) {
          return (
            <Box key={line.key} flexDirection="column">
              <Box>
                <Text>{'  '.repeat(line.depth)}</Text>
                <Text color="gizzi">✎ </Text>
                <Input
                  value={editValue}
                  onChange={setEditValue}
                  onSubmit={() => {
                    void submitEdit()
                  }}
                />
              </Box>
              <Text dimColor>{'  '.repeat(line.depth)}enter rename · empty cancels</Text>
            </Box>
          )
        }

        const confirmDeleteThisRow =
          subMode?.kind === 'confirm-delete' && subMode.key === line.key
        const reparentThisRow =
          subMode?.kind === 'reparent' && subMode.key === line.key

        return (
          <React.Fragment key={line.key}>
            <Box>
              <Text>
                {focused ? (isSelected ? '❯ ' : '  ') : ''}
                {'  '.repeat(line.depth)}
              </Text>
              <Text color={color}>{icon} </Text>
              <Text bold={statusOf(line.node) === 'RUNNING'} dimColor={dim}>
                {title}
              </Text>
              {statusOf(line.node) === 'RUNNING' && line.node.assignee && (
                <Text dimColor> ({line.node.assignee})</Text>
              )}
              {confirmDeleteThisRow && (
                <Text color="error"> ⚠ delete this node? [y/n]</Text>
              )}
              {error && (
                <Text color="error">
                  {' '}⚠ {truncateToWidth(error, Math.max(20, columns - maxTitleWidth - 25))}
                </Text>
              )}
            </Box>
            {reparentThisRow && subMode?.kind === 'reparent' && (
              <Box flexDirection="column" marginLeft={line.depth * 2}>
                <Text dimColor={subMode.index !== 0}>
                  {subMode.index === 0 ? '❯ ' : '  '}
                  {'⟡ (root)'}
                </Text>
                {subMode.candidates.map((candidate, index) => (
                  <Text key={candidate.node_id} dimColor={subMode.index !== index + 1}>
                    {subMode.index === index + 1 ? '❯ ' : '  '}
                    {truncateToWidth(candidate.title, maxTitleWidth)}
                  </Text>
                ))}
                <Text dimColor>j/k choose · enter move here · esc cancel</Text>
              </Box>
            )}
          </React.Fragment>
        )
      })}
      {hiddenCount > 0 && <Text dimColor>{` … +${hiddenCount} more`}</Text>}
      {focused && subMode === null && (
        <Text dimColor>
          {hintForActions(selectable[selectionIndex]?.actions ?? [])}
        </Text>
      )}
    </Box>
  )
}

const ROW_ACTION_HINTS: Record<RowAction, string> = {
  take: 't take',
  done: 'd done',
  fail: 'x fail',
  edit: 'e edit',
  delete: 'D delete',
  reparent: 'r reparent',
}

/** Footer hint: only the actions the currently selected row supports. */
function hintForActions(actions: RowAction[]): string {
  const keys = actions.map(action => ROW_ACTION_HINTS[action])
  return ['j/k move', ...keys, 'esc blur'].join(' · ')
}
