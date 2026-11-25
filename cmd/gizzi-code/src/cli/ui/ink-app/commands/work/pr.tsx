import * as React from 'react'
import { Box, Text } from '../../ink'
import { getCwdState } from '../../bootstrap/state.js'
import { openBrowser } from '../../utils/browser.js'
import type { LocalJSXCommandCall } from '../../types/command.js'
import { checkCounts, conflictCount, currentBranchPR, formatPrStatus, prChip, type BranchPR, type PrCheck, type PrStatusLine } from '@/cli/work/api'
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

const TONE: Record<PrStatusLine['tone'], string | undefined> = { good: 'success', bad: 'error', wait: 'warning', none: undefined }

export function PrPane({ onDone }: { onDone: () => void }): React.ReactNode {
  const { data, error } = useLoaded(async () => {
    const pr = await currentBranchPR(getCwdState())
    if (!pr) throw new Error('No pull request for this branch (or `gh` is not installed and signed in).')
    const conflicts = pr.mergeable === 'CONFLICTING' && pr.base ? await conflictCount(pr.base, getCwdState()) : undefined
    return { pr, conflicts }
  }, 20_000)
  const pr: BranchPR | undefined = data?.pr
  const n = pr && checkCounts(pr)
  return (
    <WorkPane
      title={pr ? `PR #${pr.number} ${pr.title}` : 'Pull request'}
      subtitle={
        pr &&
        `${pr.isDraft ? 'draft' : pr.state.toLowerCase()} · ${pr.branch} · ${n!.pass} passed, ${n!.fail} failed, ${n!.pending} running`
      }
      items={pr?.checks}
      header={
        pr ? (
          <Box flexDirection="column">
            {formatPrStatus(pr, data?.conflicts).map(l => (
              <Text key={l.label}>
                <Text dimColor>{l.label.padEnd(7)}</Text>
                <Text color={TONE[l.tone]}>{l.text}</Text>
              </Text>
            ))}
          </Box>
        ) : null
      }
      error={error}
      empty="This PR has no checks."
      hint="↑/↓ choose · enter open check · o open PR"
      onClose={onDone}
      onKey={(input, check) => {
        if (input === 'o' && pr) void openBrowser(pr.url)
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
        ? [
            `${prChip(pr)} · ${pr.title} · ${pr.url}`,
            ...formatPrStatus(pr, pr.mergeable === 'CONFLICTING' && pr.base ? await conflictCount(pr.base, getCwdState()) : undefined).map(
              l => `  ${l.label.padEnd(7)}${l.text}`,
            ),
            ...pr.checks.map(c => `  ${MARK[c.state].mark} ${c.name}`)].join('\n')
        : 'No pull request for this branch.',
      { display: 'system' },
    )
    return null
  }
  return <PrPane onDone={() => onDone(undefined, { display: 'skip' })} />
}
