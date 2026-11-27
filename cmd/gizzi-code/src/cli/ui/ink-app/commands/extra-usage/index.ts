import { getIsNonInteractiveSession } from '../../bootstrap/state'
import type { Command } from '../../commands'
import { isOverageProvisioningAllowed } from '../../utils/auth'
import { isEnvTruthy } from '../../utils/envUtils'

function isExtraUsageAllowed(): boolean {
  if (isEnvTruthy(process.env.DISABLE_EXTRA_USAGE_COMMAND)) {
    return false
  }
  return isOverageProvisioningAllowed()
}

export const extraUsage = {
  type: 'local-jsx',
  name: 'extra-usage',
  description: 'Configure extra usage to keep working when limits are hit',
  // Hidden alias of /status: shows its result on the Usage tab (plan D4).
  isHidden: true,
  isEnabled: () => isExtraUsageAllowed() && !getIsNonInteractiveSession(),
  load: () => import('./extra-usage.js'),
} satisfies Command

export const extraUsageNonInteractive = {
  type: 'local',
  name: 'extra-usage',
  supportsNonInteractive: true,
  description: 'Configure extra usage to keep working when limits are hit',
  isEnabled: () => isExtraUsageAllowed() && getIsNonInteractiveSession(),
  isHidden: true,
  load: () => import('./extra-usage-noninteractive.js'),
} satisfies Command
