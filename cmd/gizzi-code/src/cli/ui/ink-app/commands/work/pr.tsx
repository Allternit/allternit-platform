import * as React from 'react'
import { Text } from '../../ink'
import { getCwdState } from '../../bootstrap/state.js'
import { openBrowser } from '../../utils/browser.js'
import type { LocalJSXCommandCall } from '../../types/command.js'
import { checkCounts, currentBranchPR, prChip, type PrCheck } from '@/cli/work/api'
import { useLoaded, WorkPane } from './WorkPane'

/**
 * /pr — the current branch's pull request and its checks, refreshed every
 * 20 seconds while open (`gh pr view`, the source the engine's Linked PRs
 * use). Enter opens the selected check; o opens the PR.
 */
const MARK: Record<PrCheck['state'], { mark: string; color?: string }> = {
  pass: { mark: '✓', color: 'success' },
  fail: { mark: '✗', color: 'error' },
  pending: { mark: '●', color: 'warning' },
  skip: { mark: '–' },
}

export function PrPane({ onDone }: { onDone: () => void }): React.ReactNode {
  const { data, error } = useLoaded(async () => {
    const pr = await currentBranchPR(getCwdState())
    if (!pr) throw new Error('No pull request for this branch (or `gh` is not installed and signed in).')
    return pr
  }, 20_000)
  const n = data && checkCounts(data)
  return (
    <WorkPane
      title={data ? `PR #${data.number} ${data.title}` : 'Pull request'}
      subtitle={
        data &&
        `${data.isDraft ? 'draft' : data.state.toLowerCase()} · ${data.branch} · ${n!.pass} passed, ${n!.fail} failed, ${n!.pending} running`
      }
      items={data?.checks}
      error={error}
      empty="This PR has no checks."
      hint="↑/↓ choose · enter open check · o open PR"
      onClose={onDone}
      onKey={(input, check) => {
        if (input === 'o' && data) void openBrowser(data.url)
        if (input === '\r' && check?.url) void openBrowser(check.url)
      }}
      renderRow={c => (
        <Text wrap="truncate-end">
          <Text color={MARK[c.state].color}>{MARK[c.state].mark}</Text> {c.name}
        </Text>
      )}
    />
  )
}

export const call: LocalJSXCommandCall = async (onDone, context) => {
  if (context.options?.isNonInteractiveSession) {
    const pr = await currentBranchPR(getCwdState())
    onDone(
      pr
        ? [`${prChip(pr)} · ${pr.title} · ${pr.url}`, ...pr.checks.map(c => `  ${MARK[c.state].mark} ${c.name}`)].join('\n')
        : 'No pull request for this branch.',
      { display: 'system' },
    )
    return null
  }
  return <PrPane onDone={() => onDone(undefined, { display: 'skip' })} />
}
