import type { Command } from '../../commands'

const viewPlan = {
  type: 'local-jsx',
  name: 'view-plan',
  aliases: ['show-plan', 'plan-view'],
  description: 'Open a preview of the current saved plan without entering plan mode',
  // Hidden alias: `/plan view` is the visible form (overhaul plan D4).
  isHidden: true,
  immediate: true,
  load: () => import('./view-plan.js'),
} satisfies Command

export default viewPlan
