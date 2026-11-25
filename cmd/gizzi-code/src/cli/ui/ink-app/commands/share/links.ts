import { formatLinks, listLinks, pauseLink, revokeLink } from '@/cli/apps/client'

/** `/share links | pause <id> | resume <id> | stop <id>`: your share links, with pause and revoke. */
export async function linksView(args: string[]): Promise<string> {
  const [verb, id] = args
  try {
    if (!verb || verb === 'links' || verb === 'list') {
      const links = await listLinks(false)
      const tail = links.length ? '\n\n/share pause <id>   /share resume <id>   /share stop <id>' : ''
      return formatLinks(links) + tail
    }
    if (!id) return `Usage: /share ${verb} <id>`
    if (verb === 'pause') return (await pauseLink(id, true), `Paused ${id}.`)
    if (verb === 'resume') return (await pauseLink(id, false), `Resumed ${id}.`)
    if (verb === 'stop') return (await revokeLink(id), `Stopped ${id}.`)
    return 'Usage: /share links | pause <id> | resume <id> | stop <id>'
  } catch (e) {
    return e instanceof Error ? e.message : String(e)
  }
}

export const LINK_VERBS = new Set(['links', 'list', 'pause', 'resume', 'stop'])
