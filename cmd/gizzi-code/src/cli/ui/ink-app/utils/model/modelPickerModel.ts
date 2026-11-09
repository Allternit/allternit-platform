/**
 * Pure model-picker logic for the TUI /model organizer — no Ink, no React.
 * Ported from the desktop ModelPickerPopover (allternit-ai
 * src/components/chat/ModelPickerPopover.tsx): vendor inference, context
 * formatting, provider grouping, filtering, favorites ordering, and quota
 * summarization. Kept dependency-free so it is unit-testable on its own.
 */

/** Minimal option shape the picker consumes (matches ModelOption). */
export interface PickerOption {
  value: string
  label: string
  description?: string
}

/** Per-option provider metadata, looked up by option value (`providerId/modelId`). */
export interface PickerProviderMeta {
  providerId: string
  providerName: string
  /** Discovery source: 'platform' | 'subprocess' | 'local' | 'subscription' | 'plugin', or 'api' for a gizzi.json key. */
  source: string
  /** The shared offer rule's kind (runtime/providers/offer.ts), which picks the section. */
  kind?: 'cloud' | 'subscription' | 'api' | 'local'
  /** Context window in tokens, when discovery reported it. */
  context?: number
  /** Max output tokens, when discovery reported it. */
  output?: number
}

export type PickerRow =
  | {
      kind: 'header'
      key: string
      title: string
      /** Owning provider for quota lookup, when the section maps to one. */
      providerId?: string
    }
  | {
      kind: 'option'
      key: string
      value: string
      label: string
      description?: string
      providerId?: string
      favorite: boolean
      contextLabel: string | null
      badges: string[]
    }

/** Upstream vendor for a model routed through Allternit Cloud. */
export function inferVendor(model: { id: string; name?: string }): string {
  const s = `${model.id} ${model.name ?? ''}`.toLowerCase()
  if (/claude|anthropic|opus|sonnet|haiku/.test(s)) return 'anthropic'
  if (/\bgpt|openai|\bo[134]\b|o[134]-|codex/.test(s)) return 'openai'
  if (/gemini|gemma|google/.test(s)) return 'google'
  if (/grok|xai/.test(s)) return 'xai'
  if (/deepseek/.test(s)) return 'deepseek'
  if (/mistral|mixtral|codestral|magistral/.test(s)) return 'mistral'
  if (/kimi|moonshot/.test(s)) return 'kimi'
  if (/qwen|qwq/.test(s)) return 'qwen'
  if (/glm|zhipu|z-ai|zai/.test(s)) return 'zai'
  if (/llama|meta/.test(s)) return 'meta'
  return 'other'
}

const VENDOR_DISPLAY: Record<string, string> = {
  anthropic: 'Anthropic',
  openai: 'OpenAI',
  google: 'Google',
  xai: 'xAI',
  deepseek: 'DeepSeek',
  mistral: 'Mistral',
  kimi: 'Kimi',
  qwen: 'Qwen',
  zai: 'Z.ai',
  meta: 'Meta',
  other: 'Other',
}

export function vendorDisplayName(vendor: string): string {
  return VENDOR_DISPLAY[vendor] ?? 'Other'
}

/** 200000 → "200K", 1000000 → "1.0M", 8192 → "8K"; null when unknown. */
export function formatContext(value: unknown): string | null {
  const n =
    typeof value === 'number'
      ? value
      : typeof value === 'string'
        ? Number(value)
        : NaN
  if (!Number.isFinite(n) || n <= 0) return null
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`
  if (n >= 1000) return `${Math.round(n / 1000)}K`
  return String(n)
}

/**
 * Short capability tags. Discovery doesn't carry a capabilities array, so
 * these are inferred from the model id/name and the provider source; a badge
 * is only emitted when there is a positive signal (never guessed).
 */
export function capabilityBadges(model: {
  id: string
  name?: string
  source?: string
}): string[] {
  const s = `${model.id} ${model.name ?? ''}`.toLowerCase()
  const badges: string[] = []
  if (/reason|think|\br1\b|\bo[134]\b|qwq|deepseek-r/.test(s)) badges.push('reasoning')
  if (/vision|\bvl\b|\b4v\b|pixtral|llava/.test(s)) badges.push('vision')
  if (model.source === 'local') badges.push('local')
  return badges
}

export function toggleFavorite(favorites: string[], value: string): string[] {
  return favorites.includes(value)
    ? favorites.filter(v => v !== value)
    : [...favorites, value]
}

/** Loose QuotaResult shape — mirrored from src/runtime/providers/quota. */
export interface PickerQuotaResult {
  status: 'ok' | 'unsupported' | 'signed-out' | 'expired' | 'error'
  quota?: {
    windows: Array<{ id: string; label: string; usedRatio: number }>
  }
  message?: string
}

/**
 * "5h: 62% left" for the tightest reported window, or null when the provider
 * reports nothing usable — never a fabricated number.
 */
export function quotaSummary(
  result: PickerQuotaResult | undefined,
): string | null {
  if (!result || result.status !== 'ok' || !result.quota) return null
  const windows = result.quota.windows
  if (!windows.length) return null
  const tightest = windows.reduce((a, b) =>
    b.usedRatio > a.usedRatio ? b : a,
  )
  const left = Math.round((1 - tightest.usedRatio) * 100)
  return `${tightest.id}: ${left}% left`
}

/**
 * Marker for a picker section/detail: the real summary when the provider
 * reported one, an explicit "quota n/a" when the provider has no quota API
 * (hasFetcher false), a short honest status for known fetch outcomes, and
 * null only while a fetch is still in flight — never a fabricated number.
 */
export function quotaMarker(
  result: PickerQuotaResult | undefined,
  hasFetcher: boolean,
): string | null {
  const summary = quotaSummary(result)
  if (summary) return summary
  if (!hasFetcher) return 'quota n/a'
  if (!result) return null // fetch in flight — say nothing yet
  switch (result.status) {
    case 'signed-out':
      return 'quota: signed out'
    case 'expired':
      return 'quota: sign-in expired'
    case 'error':
      return 'quota unavailable'
    default:
      return null
  }
}

function modelIdOf(value: string): string {
  const idx = value.indexOf('/')
  return idx === -1 ? value : value.slice(idx + 1)
}

function matchesQuery(
  option: PickerOption,
  meta: PickerProviderMeta | undefined,
  query: string,
): boolean {
  if (!query) return true
  const vendor =
    meta?.source === 'platform'
      ? vendorDisplayName(inferVendor({ id: modelIdOf(option.value), name: option.label }))
      : ''
  const hay =
    `${option.label} ${option.value} ${option.description ?? ''} ${meta?.providerName ?? ''} ${vendor}`.toLowerCase()
  return hay.includes(query)
}

/**
 * Group options into sectioned rows, in Desktop's order: Allternit Cloud
 * split by upstream vendor, one section per subscription (CLI sign-ins,
 * Subscription Fabric), one per connected API key, then Local, then Other
 * for anything without provider metadata. Favorites float to the top within
 * their section; sections of the same kind keep their order in `options`.
 */
export function buildPickerRows(
  options: PickerOption[],
  opts: {
    metaFor?: (value: string) => PickerProviderMeta | undefined
    favorites?: string[]
    query?: string
  } = {},
): { rows: PickerRow[]; matched: number; total: number } {
  const favorites = opts.favorites ?? []
  const query = (opts.query ?? '').trim().toLowerCase()

  // Desktop's section order: Allternit Cloud, your subscriptions, API keys you
  // connected, local runtimes, then anything without provider metadata.
  type Section = { key: string; title: string; providerId?: string; rank: number; rows: PickerRow[] }
  const sections: Section[] = []
  const byKey = new Map<string, Section>()

  const kindOf = (meta: PickerProviderMeta | undefined, option: PickerOption) => {
    if (meta?.kind) return meta.kind
    if (meta?.source === 'platform') return 'cloud'
    if (meta?.source === 'local' || /· local$/.test(option.description ?? '')) return 'local'
    if (meta?.source === 'api') return 'api'
    return meta ? 'subscription' : undefined
  }

  const sectionFor = (
    option: PickerOption,
    meta: PickerProviderMeta | undefined,
  ): Section => {
    let key: string
    let title: string
    let providerId: string | undefined
    let rank: number
    const kind = kindOf(meta, option)
    if (kind === 'cloud') {
      const vendor = inferVendor({ id: modelIdOf(option.value), name: option.label })
      key = `cloud:${vendor}`
      title =
        vendor === 'other'
          ? 'Allternit Cloud'
          : `${vendorDisplayName(vendor)} · via Allternit Cloud`
      providerId = meta?.providerId
      rank = 0
    } else if (kind === 'subscription') {
      key = `sub:${meta!.providerId}`
      title = `${meta!.providerName} · your subscription`
      providerId = meta!.providerId
      rank = 1
    } else if (kind === 'api') {
      key = `api:${meta!.providerId}`
      title = `${meta!.providerName} · API key`
      providerId = meta!.providerId
      rank = 2
    } else if (kind === 'local') {
      key = 'local'
      title = 'Local'
      providerId = meta?.providerId
      rank = 3
    } else {
      key = 'other'
      title = 'Other'
      rank = 4
    }
    let section = byKey.get(key)
    if (!section) {
      section = { key, title, providerId, rank, rows: [] }
      byKey.set(key, section)
      sections.push(section)
    }
    return section
  }

  let matched = 0
  for (const option of options) {
    const meta = opts.metaFor?.(option.value)
    if (!matchesQuery(option, meta, query)) continue
    matched++
    const section = sectionFor(option, meta)
    section.rows.push({
      kind: 'option',
      key: `opt:${option.value}`,
      value: option.value,
      label: option.label,
      description: option.description,
      providerId: meta?.providerId,
      favorite: favorites.includes(option.value),
      contextLabel: meta?.context ? formatContext(meta.context) : null,
      badges: capabilityBadges({
        id: modelIdOf(option.value),
        name: option.label,
        source: meta?.source,
      }),
    })
  }

  // Favorites first within each section, preserving original order otherwise.
  const rows: PickerRow[] = []
  sections.sort((a, b) => a.rank - b.rank)
  for (const section of sections) {
    section.rows.sort((a, b) => Number(b.kind === 'option' && b.favorite) - Number(a.kind === 'option' && a.favorite))
    rows.push({
      kind: 'header',
      key: `hdr:${section.key}`,
      title: section.title,
      providerId: section.providerId,
    })
    rows.push(...section.rows)
  }
  return { rows, matched, total: options.length }
}

/** Values of selectable rows only (headers are never focusable). */
export function selectableValues(rows: PickerRow[]): string[] {
  return rows.flatMap(r => (r.kind === 'option' ? [r.value] : []))
}

/**
 * Slice of rows to render such that the focused row stays visible. The
 * window is centered on the focus where possible and clamped to the list.
 */
export function visibleWindow<T>(rows: T[], focusIndex: number, count: number): T[] {
  if (rows.length <= count) return rows
  const half = Math.floor(count / 2)
  const start = Math.min(
    Math.max(0, focusIndex - half),
    Math.max(0, rows.length - count),
  )
  return rows.slice(start, start + count)
}
