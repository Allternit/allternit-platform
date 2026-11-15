import * as React from 'react'
import { useState } from 'react'
import { Box, Text } from '../../ink'
import type { LocalJSXCommandCall } from '../../types/command.js'
import {
  answerApproval,
  answerQuestion,
  approvalTarget,
  listDecisions,
  rejectQuestion,
  type Decision,
} from '@/cli/work/api'
import { useLoaded, WorkPane } from './WorkPane'

/**
 * /decisions — every question and approval an agent is waiting on, across
 * all sessions on this machine's engine (terminal sessions, Desktop chats,
 * bot chats, Factory workers): the same queue as Desktop's Tasks → Decisions.
 * Answers go back through the engine, so the asking session continues
 * wherever it runs.
 */
function source(d: Decision): string {
  return d.session ?? d.sessionID
}

export function DecisionsPane({ onDone }: { onDone: (msg?: string) => void }): React.ReactNode {
  const { data, error, reload } = useLoaded(listDecisions, 5_000)
  // Picks so far for a multi-question decision, keyed by decision id.
  const [picks, setPicks] = useState<Record<string, string[][]>>({})
  const [notice, setNotice] = useState<string>()
  const [failure, setFailure] = useState<string>()

  const done = (p: Promise<unknown>, what: string) =>
    p.then(
      () => {
        setNotice(what)
        setFailure(undefined)
        reload()
      },
      e => setFailure(e instanceof Error ? e.message : String(e)),
    )

  const onKey = (input: string, d: Decision | undefined) => {
    if (!d) return
    if (d.kind === 'approval') {
      if (input === 'y' || input === '\r') void done(answerApproval(d.id, 'once'), `Allowed once · ${source(d)}`)
      else if (input === 'a') void done(answerApproval(d.id, 'always'), `Always allowed · ${source(d)}`)
      else if (input === 'n') void done(answerApproval(d.id, 'reject'), `Denied · ${source(d)}`)
      return
    }
    if (input === 'x') return void done(rejectQuestion(d.id), `Dismissed · ${source(d)}`)
    const n = Number(input)
    const so = picks[d.id] ?? []
    const q = d.questions[so.length]
    if (!q || !Number.isInteger(n) || n < 1 || n > q.options.length) return
    const next = [...so, [q.options[n - 1]!.label]]
    if (next.length < d.questions.length) return setPicks(p => ({ ...p, [d.id]: next }))
    setPicks(p => ({ ...p, [d.id]: [] }))
    void done(answerQuestion(d.id, next), `Answered · ${source(d)}`)
  }

  return (
    <WorkPane
      title="Decisions"
      subtitle={data ? `${data.length} waiting for you` : undefined}
      items={data}
      error={failure ?? (error && `Couldn't reach the Allternit engine (${error}). Open Allternit Desktop or run \`gizzi serve\`.`)}
      empty="Nothing is waiting for you."
      notice={notice}
      hint="↑/↓ choose · 1-9 answer · x dismiss · y allow · a always · n deny"
      onClose={() => onDone()}
      onKey={onKey}
      renderRow={d => (
        <Text wrap="truncate-end">
          <Text color={d.kind === 'approval' ? 'warning' : 'suggestion'}>{d.kind === 'approval' ? 'approve' : 'question'}</Text>
          <Text dimColor> {source(d)} · </Text>
          {d.kind === 'approval' ? `${d.permission}: ${approvalTarget(d)}` : (d.questions[0]?.header ?? d.questions[0]?.question)}
        </Text>
      )}
      detail={d => {
        if (d.kind === 'approval') {
          return (
            <>
              <Text>
                Allow <Text bold>{d.permission}</Text>: {approvalTarget(d)}?
              </Text>
              <Text dimColor>y allow once · a always · n deny</Text>
            </>
          )
        }
        const step = (picks[d.id] ?? []).length
        const q = d.questions[step]
        if (!q) return null
        return (
          <>
            <Text>
              {d.questions.length > 1 ? `(${step + 1}/${d.questions.length}) ` : ''}
              {q.question}
            </Text>
            {q.options.map((o, i) => (
              <Box key={i}>
                <Text color="suggestion">{`  ${i + 1}. `}</Text>
                <Text>{o.label}</Text>
                {o.description ? <Text dimColor> — {o.description}</Text> : null}
              </Box>
            ))}
          </>
        )
      }}
    />
  )
}

export const call: LocalJSXCommandCall = async (onDone, context) => {
  if (context.options?.isNonInteractiveSession) {
    const list = await listDecisions().catch(() => undefined)
    if (!list) return onDone('The Allternit engine is not reachable.', { display: 'system' }), null
    onDone(
      list.length
        ? list
            .map(d => `${d.kind === 'approval' ? 'approve' : 'question'} · ${source(d)} · ${d.kind === 'approval' ? approvalTarget(d) : d.questions[0]?.question}`)
            .join('\n')
        : 'Nothing is waiting for you.',
      { display: 'system' },
    )
    return null
  }
  return <DecisionsPane onDone={msg => onDone(msg, { display: msg ? 'system' : 'skip' })} />
}
