import * as React from 'react'
import { Text } from '../../ink'
import type { LocalJSXCommandCall } from '../../types/command.js'
import { listProjects, PROJECT_STATE_LABELS, type Project, type ProjectState } from '@/cli/work/api'
import { useLoaded, WorkPane } from './WorkPane'

/**
 * /projects — the account's projects with the same status chips Desktop's
 * Projects shows (status page `state` + `next_action`, allternit-api
 * `/api/v1/cowork/projects`). Active first, then by last update.
 */
const CHIP_COLOR: Record<ProjectState, string> = {
  on_track: 'success',
  at_risk: 'warning',
  blocked: 'error',
  needs_decision: 'suggestion',
  done: 'inactive',
}

function chip(p: Project): { text: string; color?: string } {
  if (p.lifecycle === 'closed') return { text: 'Closed', color: 'inactive' }
  if (!p.state) return { text: 'No status' }
  return { text: PROJECT_STATE_LABELS[p.state], color: CHIP_COLOR[p.state] }
}

async function load(): Promise<Project[]> {
  const list = await listProjects()
  return list.sort(
    (a, b) =>
      Number(a.lifecycle === 'closed') - Number(b.lifecycle === 'closed') ||
      (b.updatedAt ?? '').localeCompare(a.updatedAt ?? ''),
  )
}

export const call: LocalJSXCommandCall = async (onDone, context) => {
  if (context.options?.isNonInteractiveSession) {
    const list = await load().catch((e: Error) => e)
    onDone(
      list instanceof Error
        ? `Couldn't load projects: ${list.message}`
        : list.map(p => `[${chip(p).text}] ${p.title}${p.nextAction ? ` — next: ${p.nextAction}` : ''}`).join('\n') ||
            'No projects yet.',
      { display: 'system' },
    )
    return null
  }
  function ProjectsPane() {
    const { data, error } = useLoaded(load)
    return (
      <WorkPane
        title="Projects"
        subtitle={data ? `${data.filter(p => p.lifecycle === 'active').length} active` : undefined}
        items={data}
        error={error && `Couldn't load projects: ${error}`}
        empty="No projects yet. Create one in Allternit Desktop or with /cowork-project."
        hint="↑/↓ choose"
        onClose={() => onDone(undefined, { display: 'skip' })}
        renderRow={p => {
          const c = chip(p)
          return (
            <Text wrap="truncate-end">
              <Text color={c.color}>[{c.text}]</Text> {p.title}
              {p.kind ? <Text dimColor> · {p.kind}</Text> : null}
            </Text>
          )
        }}
        detail={p => (p.nextAction ? <Text dimColor>Next: {p.nextAction}</Text> : null)}
      />
    )
  }
  return <ProjectsPane />
}
