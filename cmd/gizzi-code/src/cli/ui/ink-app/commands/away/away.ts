import type { LocalCommandCall } from '../../types/command.js'
import { runAway } from '@/runtime/bots/floor-autopilot'

export const call: LocalCommandCall = async () => {
  return { type: 'text', value: await runAway() }
}
