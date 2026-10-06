// @ts-nocheck
// Leaf config module — intentionally minimal imports so UI components
// can read the auto-dream enabled state without dragging in the forked
// agent / task registry / message builder chain that autoDream.ts pulls in.

import { getInitialSettings } from '../../utils/settings/settings'
import { getFeatureValue_CACHED_MAY_BE_STALE } from '../analytics/growthbook'
import { serverDreamingActiveSync } from '../../../../../runtime/memory/drive/paths'
/**
 * Whether background memory consolidation should run. User setting
 * (autoDreamEnabled in settings.json) overrides the GrowthBook default
 * when explicitly set; otherwise falls through to tengu_onyx_plover.
 */
export function isAutoDreamEnabled(): boolean {
  // The Memory Drive's server-side nightly Dream owns consolidation when it
  // is on for this account; a local dream would compete with it.
  if (serverDreamingActiveSync()) return false
  const setting = getInitialSettings().autoDreamEnabled
  if (setting !== undefined) return setting
  const gb = getFeatureValue_CACHED_MAY_BE_STALE<{ enabled?: unknown } | null>(
    'tengu_onyx_plover',
    null,
  )
  return gb?.enabled === true
}
