/**
 * /status owns four jobs as tabs. The old commands stay as hidden aliases for
 * one release (overhaul plan D4, Eoj E3) and open their tab here.
 */
export const STATUS_TABS = ['Status', 'Usage', 'Cost', 'Stats'] as const
export type StatusTab = (typeof STATUS_TABS)[number]

/** Old command name → the /status tab it now opens. */
export const STATUS_ALIAS_TABS: Readonly<Record<string, StatusTab>> = {
  'session-info': 'Status',
  usage: 'Usage',
  'extra-usage': 'Usage',
  cost: 'Cost',
  stats: 'Stats',
}

/** `/status usage` (or cost, stats, status) opens that tab; anything else opens Status. */
export function statusTabFromArgs(args: string): StatusTab {
  const word = args.trim().toLowerCase()
  return STATUS_TABS.find(tab => tab.toLowerCase() === word) ?? 'Status'
}
