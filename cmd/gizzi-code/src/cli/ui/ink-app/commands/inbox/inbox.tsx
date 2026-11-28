import * as React from 'react'
import { Box, Text, useInput } from '../../ink'
import type { LocalJSXCommandCall } from '../../types/command'
import { listInbox, resolveInbox, type InboxItem } from '@/runtime/services/api/inbox'

type State =
  | { kind: 'loading' }
  | { kind: 'error'; text: string }
  | { kind: 'ready'; items: InboxItem[]; cursor: number; notice?: string }

function InboxList({ onDone }: { onDone: (text: string) => void }) {
  const [state, setState] = React.useState<State>({ kind: 'loading' })
  const busy = React.useRef(false)

  const load = React.useCallback(async (notice?: string) => {
    try {
      const items = await listInbox('needs_you')
      setState(prev => ({ kind: 'ready', items, cursor: prev.kind === 'ready' ? Math.min(prev.cursor, Math.max(0, items.length - 1)) : 0, notice }))
    } catch (e) {
      setState({ kind: 'error', text: e instanceof Error ? e.message : String(e) })
    }
  }, [])
  React.useEffect(() => { void load() }, [load])

  useInput((input, key) => {
    if (key.escape || input === 'q') {
      onDone('Inbox closed.')
      return
    }
    if (state.kind === 'error') {
      if (input === 'r') { setState({ kind: 'loading' }); void load() }
      return
    }
    if (state.kind !== 'ready' || busy.current) return
    if (input === 'r') { void load(); return }
    if (key.upArrow || input === 'k') setState({ ...state, cursor: Math.max(0, state.cursor - 1), notice: undefined })
    else if (key.downArrow || input === 'j') setState({ ...state, cursor: Math.min(state.items.length - 1, state.cursor + 1), notice: undefined })
    else if (input === 'a' || input === 'd') {
      const item = state.items[state.cursor]
      if (!item) return
      const decision = input === 'a' ? 'approve' : 'deny'
      busy.current = true
      void resolveInbox(item.id, decision)
        .then(() => load(`${decision === 'approve' ? 'Approved' : 'Denied'}: ${item.action_summary}`))
        .catch(e => setState({ ...state, notice: e instanceof Error ? e.message : String(e) }))
        .finally(() => { busy.current = false })
    }
  })

  if (state.kind === 'loading') return <Text dimColor>Loading your inbox…</Text>
  if (state.kind === 'error') {
    return (
      <Box flexDirection="column">
        <Text color="error">Couldn't read the inbox: {state.text}</Text>
        <Text dimColor>r retry · Esc close</Text>
      </Box>
    )
  }
  return (
    <Box flexDirection="column">
      <Text bold>Needs you ({state.items.length})</Text>
      {state.items.length === 0 && <Text dimColor>Nothing needs you.</Text>}
      {state.items.map((i, n) => (
        <Box key={i.id} flexDirection="column" marginTop={1}>
          <Text bold={n === state.cursor}>{n === state.cursor ? '> ' : '  '}{i.action_summary}</Text>
          <Text dimColor>    {i.action_class} · {i.urgency} · due {i.deadline} · {i.id}</Text>
        </Box>
      ))}
      {state.notice && <Text color="warning">{state.notice}</Text>}
      <Box marginTop={1}>
        <Text dimColor>↑/↓ choose · a approve · d deny · r refresh · Esc close</Text>
      </Box>
    </Box>
  )
}

/** /inbox — lists the needs-you items; you approve or deny with a key. */
export const call: LocalJSXCommandCall = async (onDone, context) => {
  if (context.options?.isNonInteractiveSession) {
    onDone('Use `gizzi inbox` outside the TUI.', { display: 'system' })
    return null
  }
  return <InboxList onDone={text => onDone(text, { display: 'system' })} />
}
