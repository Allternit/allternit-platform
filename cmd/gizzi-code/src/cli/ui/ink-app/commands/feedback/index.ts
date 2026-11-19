// @ts-nocheck
import type { Command } from '../../commands'
import { isPolicyAllowed } from '../../services/policyLimits/index'
import { isEnvTruthy } from '../../utils/envUtils'
import { isEssentialTrafficOnly } from '../../utils/privacyLevel'

const feedback = {
  aliases: ['bug'],
  type: 'local-jsx',
  name: 'feedback',
  description: `Submit feedback about Gizzi Code`,
  argumentHint: '[report | notes [delete <n>]]',
  isEnabled: () =>
    !(
      isEnvTruthy(process.env.GIZZI_CODE_USE_BEDROCK) ||
      isEnvTruthy(process.env.GIZZI_CODE_USE_VERTEX) ||
      isEnvTruthy(process.env.GIZZI_CODE_USE_FOUNDRY) ||
      isEnvTruthy(process.env.DISABLE_FEEDBACK_COMMAND) ||
      isEnvTruthy(process.env.DISABLE_BUG_COMMAND) ||
      isEssentialTrafficOnly() ||
      process.env.USER_TYPE === 'ant' ||
      !isPolicyAllowed('allow_product_feedback')
    ),
  load: () => import('./feedback.js'),
} satisfies Command
export default feedback
