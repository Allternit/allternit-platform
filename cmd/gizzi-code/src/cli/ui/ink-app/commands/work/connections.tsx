import * as React from 'react'
import { useCallback, useEffect, useState } from 'react'
import { Pane } from '../../components/design-system/Pane'
import { Box, Text, useInput } from '../../ink'
import type { LocalJSXCommandCall } from '../../types/command.js'
import { fetchCatalog, listRows } from '@/cli/commands/connect-run'
import type { CatalogEntry, Row } from '@/cli/commands/connect-model'
import {
  PAGE, canMore, deckLines, footer, gridCols, gridLines, listLines, mergeItems, nextMode, summary, type Mode,
} from '@/cli/work/connections-model'

/**
 * Connections in the terminal: List · Grid · Deck over the live catalog with
 * connected state. tab switches mode, / filters, m loads the next page in
 * place (capped), enter on a row prints the `gizzi connect <id>` to run.
 */
export function ConnectionsPane({ onDone }: { onDone: () => void }): React.ReactNode {
  const [mode, setMode] = useState<Mode>('list')
  const [entries, setEntries] = useState<CatalogEntry[]>([])
  const [rows, setRows] = useState<Row[]>([])
  const [total, setTotal] = useState(0)
  const [cursor, setCursor] = useState<string | null | undefined>()
  const [q, setQ] = useState('')
  const [draft, setDraft] = useState<string | undefined>()
  const [at, setAt] = useState(0)
  const [error, setError] = useState<string>()
  const [loading, setLoading] = useState(true)
  const [notice, setNotice] = useState<string>()

  const load = useCallback(async (query: string, from?: string) => {
    setLoading(true)
    try {
      const res = await fetchCatalog({ q: query || undefined, limit: PAGE, cursor: from })
      setEntries(prev => (from ? [...prev, ...res.entries] : res.entries))
      setTotal(res.total)
      setCursor(res.next_cursor)
      setError(undefined)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [])
  useEffect(() => { void load(q) }, [q, load])
  useEffect(() => { listRows().then(setRows, () => {}) }, [])

  const items = mergeItems(entries, rows)
  const cols = gridCols(78)
  const more = canMore(entries.length, cursor)
  const sel = Math.min(at, Math.max(0, items.length - 1))

  useInput((input, key) => {
    if (draft !== undefined) {
      if (key.escape) return setDraft(undefined)
      if (key.return) { setQ(draft); setAt(0); setDraft(undefined); return }
      if (key.backspace || key.delete) return setDraft(d => (d ?? '').slice(0, -1))
      if (!key.ctrl && !key.meta && input) setDraft(d => (d ?? '') + input)
      return
    }
    if (key.escape) return onDone()
    if (key.tab) return setMode(m => nextMode(m, key.shift ? -1 : 1))
    if (input === '/') return setDraft(q)
    if (input === 'm' && more && !loading) return void load(q, cursor ?? undefined)
    const last = items.length - 1
    if (key.leftArrow) return setAt(mode === 'grid' ? Math.max(0, sel - 1) : Math.max(0, sel - 1))
    if (key.rightArrow) return setAt(Math.min(last, sel + 1))
    if (key.upArrow) return setAt(mode === 'grid' ? Math.max(0, sel - cols) : Math.max(0, sel - 1))
    if (key.downArrow) return setAt(mode === 'grid' ? Math.min(last, sel + cols) : Math.min(last, sel + 1))
    if (key.return && items[sel]) setNotice(`Connect it with: gizzi connect ${items[sel]!.entry.id}`)
  })

  const lines = mode === 'list' ? listLines(items, sel) : mode === 'grid' ? gridLines(items, sel) : deckLines(items, sel)
  return (
    <Pane>
      <Box>
        <Text bold>Connections</Text>
        <Text dimColor> · {mode} · {summary(items, total, q)}</Text>
      </Box>
      {draft !== undefined ? <Text>Filter: {draft}▌</Text> : null}
      {error ? <Text color="warning">{error}</Text> : null}
      {loading && !items.length ? <Text dimColor>Loading…</Text> : null}
      {!loading && !error && !items.length ? <Text dimColor>{q ? `Nothing matches "${q}".` : 'No connections in the catalog.'}</Text> : null}
      {lines.map((l, i) => <Text key={i}>{l}</Text>)}
      {notice ? <Text color="success">{notice}</Text> : null}
      <Box marginTop={1}><Text dimColor>{footer(mode, more, draft !== undefined)}</Text></Box>
    </Pane>
  )
}

export const call: LocalJSXCommandCall = async (onDone, context) => {
  if (context.options?.isNonInteractiveSession) {
    onDone('Use `gizzi connect more` for the catalog in a script.', { display: 'system' })
    return null
  }
  return <ConnectionsPane onDone={() => onDone(undefined, { display: 'skip' })} />
}
