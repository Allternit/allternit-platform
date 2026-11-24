import * as React from 'react'
import { Text } from '../../ink'
import type { LocalJSXCommandCall } from '../../types/command.js'
import {
  getProjectPad,
  listProjects,
  projectPadScope,
  PROJECT_STATE_LABELS,
  type Project,
  type ProjectPadResult,
  type ProjectState,
} from '@/cli/work/api'
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

function hostOf(url?: string): string {
  try {
    return new URL(url ?? '').host
  } catch {
    return url ?? ''
  }
}

/** The pad as plain text lines: Notes (first 6 lines), Checklist, Links. */
export function padLines(r: ProjectPadResult): string[] {
  if (r.unsupported) return ['Update your computer to sync pads']
  if (!r.pad) return ['No pad yet']
  const d = r.pad.data ?? {}
  const out: string[] = []
  const notes = (d.notes ?? '').split('\n').map(l => l.trimEnd()).filter(l => l.trim())
  if (notes.length) out.push('Notes', ...notes.slice(0, 6).map(l => `  ${l}`), ...(notes.length > 6 ? ['  …'] : []))
  const items = d.checklist ?? []
  if (items.length) out.push('Checklist', ...items.map(i => `  [${i.done ? 'x' : ' '}] ${i.text ?? ''}`))
  const links = d.links ?? []
  if (links.length) out.push('Links', ...links.map(l => `  ${l.title || hostOf(l.url)} — ${hostOf(l.url)}`))
  return out.length ? out : ['No pad yet']
}

/** Pad text for the selected project. Latest threads are not in the data this view loads, so they are not shown. */
export function PadText({ result }: { result: ProjectPadResult }): React.ReactNode {
  return (
    <>
      {padLines(result).map((l, i) => (
        <Text key={i} dimColor={l.startsWith('  ') || l.startsWith('No pad') || l.startsWith('Update')} bold={/^[A-Z][a-z]+$/.test(l)}>
          {l}
        </Text>
      ))}
    </>
  )
}

function ProjectPadDetail({ project }: { project: Project }): React.ReactNode {
  // The list carries no project mode, so the scope uses "chat".
  const { data, error } = useLoaded(() => getProjectPad(projectPadScope(project.id)))
  if (error) return <Text dimColor>Couldn't load pad: {error}</Text>
  if (!data) return <Text dimColor>Loading pad…</Text>
  return <PadText result={data} />
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
        : (
            await Promise.all(
              list.map(async p => {
                const pad = await getProjectPad(projectPadScope(p.id)).catch(() => undefined)
                const head = `[${chip(p).text}] ${p.title}${p.nextAction ? ` — next: ${p.nextAction}` : ''}`
                return pad ? [head, ...padLines(pad).map(l => `  ${l}`)].join('\n') : head
              }),
            )
          ).join('\n') || 'No projects yet.',
      { display: 'system' },
    )
    return null
  }
  function ProjectsPane() {
    const { data, error } = useLoaded(load)
    return (
      <WorkPane
        title="Projects"
        blockTitle
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
        detail={p => (
          <>
            {p.nextAction ? <Text dimColor>Next: {p.nextAction}</Text> : null}
            <ProjectPadDetail key={p.id} project={p} />
          </>
        )}
      />
    )
  }
  return <ProjectsPane />
}
