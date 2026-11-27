import { CLOUD_BILLING_URL } from "@/shared/util/provider-error"
import type { ModelList } from "@/runtime/providers/model-list"
import { brainAccessFor, lockedNotice, offlineNotice, type BrainAccess } from "@/cli/ui/ink-app/utils/model/modelPickerModel"

export interface ModelsLine {
  /** `row` is a model, `offline` is a provider that did not answer. */
  type: "row" | "offline"
  text: string
  /** Access badge (plan / key / local / credits); the same words the picker shows. */
  access?: BrainAccess
  locked?: { reason: string; action?: string }
  current?: boolean
}

/**
 * The lines `gizzi models` prints for one provider of a section: one line per
 * visible model with its access badge and, when locked, the reason; hidden
 * rows are skipped; an offline provider is a single "<name> — offline · <fix>" line.
 */
export function formatProviderLines(
  kind: ModelList.Section["kind"],
  provider: ModelList.ProviderRows,
  billingUrl = CLOUD_BILLING_URL,
): ModelsLine[] {
  const access = brainAccessFor(kind)
  if (provider.offline) {
    return [{ type: "offline", text: `${provider.name.replace(/ — offline$/, "")} — offline · ${provider.offline.fix}` }]
  }
  const lines: ModelsLine[] = []
  for (const row of provider.models) {
    if (row.hidden) continue
    const tags: string[] = []
    if (access) tags.push(access)
    if (row.default) tags.push("default")
    if (row.current) tags.push("current")
    if (provider.signedIn === false) tags.push("not signed in")
    const text = row.locked
      ? `${row.ref}  ${tags.join(" ")}  locked: ${lockedNotice(row.locked, billingUrl)}`
      : `${row.ref}  ${tags.join(" ")}`
    lines.push({ type: "row", text, access, locked: row.locked, current: row.current })
  }
  return lines
}

/** JSON rows for `gizzi models --json`: id, section, access, locked, offline. */
export function jsonRows(sections: ModelList.Section[]) {
  return sections.flatMap((s) =>
    s.providers.flatMap((p) =>
      p.offline
        ? [{ id: p.id, section: s.kind, access: brainAccessFor(s.kind) ?? null, locked: null, offline: p.offline }]
        : p.models
            .filter((m) => !m.hidden)
            .map((m) => ({ id: m.ref, section: s.kind, access: brainAccessFor(s.kind) ?? null, locked: m.locked ?? null, offline: null })),
    ),
  )
}
