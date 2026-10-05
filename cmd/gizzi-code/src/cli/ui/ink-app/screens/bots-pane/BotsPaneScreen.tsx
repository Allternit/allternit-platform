// @ts-nocheck
/**
 * BotsPaneScreen — full-screen bots roster (Phase B5, `/bots`).
 *
 * Mounted by REPL when AppState.screen === 'bots'. Lists roster rows from
 * getBotRosterRows() (presence + canonical-chat + unread state owned by the
 * runtime bots modules):
 *
 *   ↑/↓ (or k/j)  move selection          Enter  open canonical chat
 *   n             create bot (name/title)  d      delete (inline y/n)
 *   r             refresh                  q/Esc  back to REPL
 *   f             the Allternit Factory floor
 *
 * Delete confirm idiom mirrors the dashboard's armed double-press: a bot
 * with a canonical chat requires y twice; a bot without one confirms once.
 * Enter opens the bot's chat live (openBotChat): the bot is registered on the
 * platform and the terminal becomes a client of its standing thread, the same
 * conversation Desktop and the pet HUD show. Signed out or offline, it falls
 * back to the terminal-only canonical chat and says why. Either way the bot
 * is marked read.
 */
import * as React from 'react'
import { Box, Text, useInput } from '../../ink'
import { useTerminalSize } from '../../hooks/useTerminalSize'
import { useKeybinding } from '../../keybindings/useKeybinding'
import { useAppState, useSetAppState } from '../../state/AppState'
import { useRegisterOverlay } from '../../context/overlayContext'
import { getBotRosterRows } from '@/runtime/bots/bot-roster.js'
import { createBot, deleteBot, BotStoreError } from '@/runtime/bots/bot-store.js'
import { BotsRowList } from './BotsRowList'
import { handleBotsPaneKey } from './keys'
import { openBotChat } from './open-bot-chat'
import { useNotifications } from '../../../../../context/notifications'
import { truncate } from './rows'

type DeleteConfirm = { name: string; armed: boolean }

export function BotsPaneScreen(): React.ReactNode {
  const { rows: termRows, columns } = useTerminalSize()
  const setAppState = useSetAppState()
  const { addNotification } = useNotifications()

  const [rows, setRows] = React.useState([])
  const [selected, setSelected] = React.useState(0)
  const [refreshTick, setRefreshTick] = React.useState(0)
  const [mode, setMode] = React.useState<'list' | 'create'>('list')
  const [draft, setDraft] = React.useState('')
  const [deleteConfirm, setDeleteConfirm] = React.useState<DeleteConfirm | null>(null)
  const [statusError, setStatusError] = React.useState<string | null>(null)
  const [busy, setBusy] = React.useState(false)

  useRegisterOverlay('bots')

  const exitPane = React.useCallback(() => {
    setAppState(prev => (prev.screen === 'bots' ? { ...prev, screen: 'prompt' } : prev))
  }, [setAppState])

  const refresh = React.useCallback(() => setRefreshTick(t => t + 1), [])

  React.useEffect(() => {
    let cancelled = false
    getBotRosterRows()
      .then(next => {
        if (cancelled) return
        setRows(next)
        setSelected(s => Math.max(0, Math.min(s, next.length - 1)))
      })
      .catch(err => {
        if (cancelled) return
        setStatusError(err?.message || String(err))
      })
    return () => {
      cancelled = true
    }
  }, [refreshTick])

  const selectedRow = rows.length > 0 ? rows[Math.min(selected, rows.length - 1)] : undefined

  const browsing = mode === 'list' && !deleteConfirm
  useKeybinding('bots:exit', exitPane, { context: 'Bots', isActive: browsing })

  const openRow = React.useCallback(
    (name: string) => {
      setBusy(true)
      setStatusError(null)
      openBotChat(name)
        .then(outcome => {
          setBusy(false)
          if (outcome.notice) {
            addNotification({ key: 'bots-open', text: outcome.notice, color: 'warning', priority: 'immediate', timeoutMs: 8000 })
          }
          exitPane()
        })
        .catch(err => {
          setBusy(false)
          setStatusError(err?.message || String(err))
        })
    },
    [exitPane, addNotification],
  )

  const submitCreate = React.useCallback(() => {
    const trimmed = draft.trim()
    if (!trimmed) {
      setMode('list')
      return
    }
    const sp = trimmed.indexOf(' ')
    const name = sp === -1 ? trimmed : trimmed.slice(0, sp)
    const title = sp === -1 ? trimmed : trimmed.slice(sp + 1).trim() || name
    setBusy(true)
    setStatusError(null)
    createBot({ name, title })
      .then(() => {
        setBusy(false)
        setDraft('')
        setMode('list')
        openRow(name)
      })
      .catch(err => {
        setBusy(false)
        setStatusError(err instanceof BotStoreError ? err.message : err?.message || String(err))
      })
  }, [draft, openRow])

  const confirmDelete = React.useCallback(() => {
    if (!deleteConfirm) return
    if (!deleteConfirm.armed) {
      setDeleteConfirm({ ...deleteConfirm, armed: true })
      return
    }
    const name = deleteConfirm.name
    setDeleteConfirm(null)
    setBusy(true)
    setStatusError(null)
    deleteBot(name)
      .then(() => {
        setBusy(false)
        refresh()
      })
      .catch(err => {
        setBusy(false)
        setStatusError(err?.message || String(err))
      })
  }, [deleteConfirm, refresh])

  useInput((input, key) => {
    if (busy) return

    // ---- create input mode ----
    if (mode === 'create') {
      if (key.escape) {
        setDraft('')
        setMode('list')
        return
      }
      if (key.return) {
        submitCreate()
        return
      }
      if (key.backspace || key.delete) {
        setDraft(d => d.slice(0, -1))
        return
      }
      if (key.ctrl || key.meta) return
      if (input) setDraft(d => d + input)
      return
    }

    // ---- delete confirm (repo idiom: inline y/n, Esc cancels) ----
    if (deleteConfirm) {
      if (input === 'y') {
        confirmDelete()
      } else if (input === 'n' || key.escape) {
        setDeleteConfirm(null)
      }
      return
    }

    // ---- list focus ----
    const handled = handleBotsPaneKey(input, key, {
      moveUp: () => setSelected(s => Math.max(0, s - 1)),
      moveDown: () => setSelected(s => Math.min(Math.max(0, rows.length - 1), s + 1)),
      openSelected: () => {
        if (selectedRow) openRow(selectedRow.name)
      },
      createNew: () => {
        setDraft('')
        setMode('create')
      },
      deleteSelected: () => {
        if (!selectedRow) return
        // Bots with a canonical chat get the dashboard-style double confirm.
        setDeleteConfirm({ name: selectedRow.name, armed: !selectedRow.hasCanonicalChat })
      },
      refresh,
      openFactory: () => setAppState(prev => (prev.screen === 'bots' ? { ...prev, screen: 'factory' } : prev)),
      exit: exitPane,
    })
    if (!handled && key.escape) exitPane()
  })

  const activeCount = rows.filter(r => r.active).length
  const unreadCount = rows.reduce((n, r) => n + (r.unreadCount > 0 ? 1 : 0), 0)

  return (
    <Box flexDirection="column" width={columns} height={termRows} paddingX={1}>
      <Box flexDirection="row" justifyContent="space-between">
        <Text bold color="gizzi">
          Gizzi Code · Bots
        </Text>
        <Text dimColor>
          {rows.length} bot{rows.length === 1 ? '' : 's'}
          {activeCount > 0 ? ` · ${activeCount} active` : ''}
          {unreadCount > 0 ? ` · ${unreadCount} unread` : ''}
        </Text>
      </Box>
      {deleteConfirm ? (
        <Box flexDirection="column" marginTop={1}>
          <Text color="error">
            {deleteConfirm.armed
              ? `Delete bot '${deleteConfirm.name}'? This cannot be undone. [y/n]`
              : `Bot '${deleteConfirm.name}' has a canonical chat — press y again to confirm delete. [y/n]`}
          </Text>
        </Box>
      ) : (
        <BotsRowList rows={rows} selectedIndex={selected} />
      )}
      <Box flexDirection="column" marginTop={1}>
        {mode === 'create' ? (
          <Box flexDirection="row" opaque>
            <Text color="gizzi">{'❯ '}</Text>
            <Text color="text">
              {draft}
              {'▌'}
            </Text>
          </Box>
        ) : null}
        {statusError ? (
          <Text color="error" wrap="truncate-end">
            {truncate(statusError, columns - 2)}
          </Text>
        ) : null}
        <Box flexDirection="row" justifyContent="space-between">
          <Text dimColor>
            {mode === 'create'
              ? 'Enter create <name>[ title] · Esc cancel'
              : busy
                ? 'working…'
                : '↑/↓ select · Enter open chat · n new · d delete · r refresh · f factory · q/Esc back'}
          </Text>
          <Text dimColor>/bots</Text>
        </Box>
      </Box>
    </Box>
  )
}

export default BotsPaneScreen
