import type { LocalCommandCall } from '../../types/command.js'
import { runAutopilot } from '@/runtime/bots/floor-autopilot'

export const call: LocalCommandCall = async args => {
  return { type: 'text', value: await runAutopilot(args) }
}
