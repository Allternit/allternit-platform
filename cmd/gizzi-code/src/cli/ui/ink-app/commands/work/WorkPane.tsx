import * as React from 'react'
import { useCallback, useEffect, useRef, useState } from 'react'
import { Pane } from '../../components/design-system/Pane'
import { Box, Text, useInput } from '../../ink'

/**
 * Shared shell for the work panes (/decisions, /pr, /projects, /routines):
 * loads (and optionally re-polls) a list, moves a selection with ↑/↓, hands
 * every other key to the pane, closes on esc. Errors and empty lists say so
 * plainly instead of rendering nothing.
 */
export function useLoaded<T>(load: () => Promise<T>, pollMs?: number) {
  const [data, setData] = useState<T | undefined>()
  const [error, setError] = useState<string | undefined>()
  const [tick, setTick] = useState(0)
  const loadRef = useRef(load)
  loadRef.current = load
  useEffect(() => {
    let stopped = false
    let timer: ReturnType<typeof setTimeout> | undefined
    const run = async () => {
      try {
        const next = await loadRef.current()
        if (stopped) return
        setData(next)
        setError(undefined)
      } catch (e) {
        if (!stopped) setError(e instanceof Error ? e.message : String(e))
      } finally {
        if (!stopped && pollMs) timer = setTimeout(run, pollMs)
      }
    }
    void run()
    return () => {
      stopped = true
      if (timer) clearTimeout(timer)
    }
  }, [tick, pollMs])
  const reload = useCallback(() => setTick(t => t + 1), [])
  return { data, error, reload }
}

export function WorkPane<T>({
  title,
  subtitle,
  items,
  error,
  empty,
  hint,
  renderRow,
  detail,
  onKey,
  onClose,
  notice,
}: {
  title: string
  subtitle?: string
  items: T[] | undefined
  error?: string
  empty: string
  hint: string
  renderRow: (item: T, selected: boolean) => React.ReactNode
  detail?: (item: T) => React.ReactNode
  onKey?: (input: string, item: T | undefined, index: number) => void
  onClose: () => void
  notice?: string
}): React.ReactNode {
  const [index, setIndex] = useState(0)
  const count = items?.length ?? 0
  const at = Math.min(index, Math.max(0, count - 1))
  useInput((input, key) => {
    if (key.escape) return onClose()
    if (key.upArrow) return setIndex(i => Math.max(0, Math.min(i, count - 1) - 1))
    if (key.downArrow) return setIndex(i => Math.min(count - 1, i + 1))
    if (key.ctrl || key.meta) return
    onKey?.(key.return ? '\r' : input, items?.[at], at)
  })
  const selected = items?.[at]
  return (
    <Pane>
      <Box>
        <Text bold>{title}</Text>
        {subtitle ? <Text dimColor> · {subtitle}</Text> : null}
      </Box>
      {error ? <Text color="warning">{error}</Text> : null}
      {!items && !error ? <Text dimColor>Loading…</Text> : null}
      {items && items.length === 0 ? <Text dimColor>{empty}</Text> : null}
      {items?.map((item, i) => (
        <Box key={i}>
          <Text color={i === at ? 'suggestion' : undefined}>{i === at ? '❯ ' : '  '}</Text>
          {renderRow(item, i === at)}
        </Box>
      ))}
      {selected && detail ? <Box marginTop={1} flexDirection="column">{detail(selected)}</Box> : null}
      {notice ? <Text color="success">{notice}</Text> : null}
      <Box marginTop={1}>
        <Text dimColor>{hint} · esc close</Text>
      </Box>
    </Pane>
  )
}

export function relTime(iso?: string | null): string {
  if (!iso) return '—'
  const ms = Date.parse(iso) - Date.now()
  if (!Number.isFinite(ms)) return '—'
  const abs = Math.abs(ms)
  const unit =
    abs < 60_000 ? `${Math.round(abs / 1000)}s` : abs < 3_600_000 ? `${Math.round(abs / 60_000)}m` : abs < 86_400_000 ? `${Math.round(abs / 3_600_000)}h` : `${Math.round(abs / 86_400_000)}d`
  return ms >= 0 ? `in ${unit}` : `${unit} ago`
}
