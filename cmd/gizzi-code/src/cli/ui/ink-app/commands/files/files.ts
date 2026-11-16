/**
 * `/files` in the TUI: read-only views of the Files drive over the same
 * client as `gizzi files` (src/cli/files/drive.ts).
 *
 *   /files                         this computer's folders
 *   /files "Cloud computer":Inbox  one folder (a sleeping computer answers from its saved list)
 *   /files versions Inbox/a.pdf    a file's versions
 *   /files trash [Folder]          what is in the trash
 *   /files search lease            find by name
 *   /files shares                  share links
 *
 * Nothing here wakes a computer or changes a file. The reply names the
 * `gizzi files` command for that, which asks before it wakes one.
 */
import type { LocalCommandResult } from '../../types/command'
import {
  FilesError,
  formatBytes,
  formatItems,
  formatTime,
  freshnessLine,
  openDrive,
  parseRemote,
  resolvePath,
} from '@/cli/files/drive'

/** Split like a shell: quotes group words ("Cloud computer":Inbox). */
export function splitArgs(s: string): string[] {
  const out: string[] = []
  let cur = ''
  let quote: string | undefined
  let any = false
  for (const ch of s) {
    if (quote) {
      if (ch === quote) quote = undefined
      else cur += ch
    } else if (ch === '"' || ch === "'") {
      quote = ch
      any = true
    } else if (/\s/.test(ch)) {
      if (cur || any) out.push(cur)
      cur = ''
      any = false
    } else cur += ch
  }
  if (cur || any) out.push(cur)
  return out
}

const WRITE_VERBS = new Set(['cat', 'get', 'put', 'cp', 'mv', 'rm', 'mkdir', 'share', 'pin', 'unpin', 'restore', 'wake', 'wake-setting', 'mount', 'unmount'])

async function view(args: string[]): Promise<string> {
  const [verb, ...rest] = args
  if (verb && WRITE_VERBS.has(verb)) {
    return `/files only browses. Run it in a terminal, where it can ask before waking a computer:\n  gizzi files ${args.map(a => (/\s/.test(a) ? JSON.stringify(a) : a)).join(' ')}`
  }
  if (verb === 'versions') {
    const p = parseRemote(rest[0] ?? '')
    const drive = await openDrive(p.computer)
    const at = await resolvePath(drive, p.segments)
    if (!at.item || at.isFolder) return `${at.name} is a folder. Versions are kept per file.`
    const versions = await drive.versions(at.item)
    if (!versions.length) return `${at.item.name}: no earlier versions`
    return [
      `${at.item.name} · ${drive.name}`,
      ...versions.map((v, i) => `${String(i + 1).padStart(2)}  ${formatTime(v.created_at).padEnd(18)} ${(v.created_by.display_name ?? v.created_by.kind).padEnd(20)} ${formatBytes(v.size)}${v.rev === at.item!.rev ? '  current' : ''}`),
      '',
      `Restore one: gizzi files versions restore ${rest[0]} <n>`,
    ].join('\n')
  }
  if (verb === 'trash') {
    const p = parseRemote(rest[0] ?? '')
    const drive = await openDrive(p.computer)
    const roots = await drive.roots()
    const pick = p.segments[0] ? roots.filter(r => r.name.toLowerCase() === p.segments[0].toLowerCase()) : roots
    const entries = (await Promise.all(pick.map(r => drive.trash(r.id).catch(() => [])))).flat()
    if (!entries.length) return `${drive.name}: trash is empty`
    const now = Date.now()
    return [
      `Trash · ${drive.name}`,
      ...entries.map(e => `${e.item.name.padEnd(36)} ${Math.max(0, Math.ceil((Date.parse(e.purge_at) - now) / 86_400_000))} days left`),
      '',
      'Bring one back: gizzi files restore <name>',
    ].join('\n')
  }
  if (verb === 'search') {
    const query = rest.join(' ').trim()
    if (!query) return 'Usage: /files search <text>'
    const drive = await openDrive()
    const r = await drive.search(query, 'name')
    const fresh = freshnessLine(r.freshness)
    return [`Search "${query}" · ${drive.name}${fresh ? `  ${fresh}` : ''}`, r.hits.length ? formatItems(r.hits.map(h => h.item), drive.device.id) : 'no matches'].join('\n')
  }
  if (verb === 'shares') {
    const drive = await openDrive(rest[0])
    const live = (await drive.listShares()).shares.filter(s => !s.revoked_at)
    if (!live.length) return `${drive.name}: no share links`
    return [`Share links · ${drive.name}`, ...live.map(s => `${s.role.padEnd(13)} ${s.expires_at ? `until ${formatTime(s.expires_at)}` : 'no expiry'}  ${s.guest_count} guests  ${s.link_url}`)].join('\n')
  }
  // ls (default)
  const target = verb === 'ls' || verb === 'list' ? rest[0] : verb
  const p = parseRemote(target ?? '')
  const drive = await openDrive(p.computer)
  if (!p.segments.length) {
    const { roots, freshness } = await drive.rootsWithFreshness()
    const fresh = freshnessLine(freshness)
    return [
      `${drive.name}${fresh ? `  ${fresh}` : ''}`,
      ...roots.filter(r => !r.parent_root_id).map(r => `${(r.name + '/').padEnd(24)} ${r.usage_bytes !== undefined ? formatBytes(r.usage_bytes) : ''}`),
      '',
      'Open a folder: /files Inbox   ·   files in this session: /context files',
    ].join('\n')
  }
  const at = await resolvePath(drive, p.segments)
  if (!at.isFolder) return formatItems([at.item!], drive.device.id)
  const { items, freshness } = await drive.list(at.folder.rootId, at.folder.itemId)
  const fresh = freshnessLine(freshness)
  return [`${drive.name} · ${p.segments.join('/')}${fresh ? `  ${fresh}` : ''}`, formatItems(items, drive.device.id)].join('\n')
}

export async function call(args: string): Promise<LocalCommandResult> {
  try {
    return { type: 'text' as const, value: await view(splitArgs(args ?? '')) }
  } catch (e) {
    const err = e instanceof FilesError ? e : undefined
    const msg = err ? `${err.code === 'internal' || err.code === 'offline' ? '' : `${err.code}: `}${err.message}` : String((e as Error)?.message ?? e)
    return { type: 'text' as const, value: msg }
  }
}
