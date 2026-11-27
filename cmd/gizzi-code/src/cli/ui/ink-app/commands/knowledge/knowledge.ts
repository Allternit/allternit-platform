import type { LocalCommandCall } from '../../types/command.js'
import { runKnowledge } from '@/runtime/knowledge/client'

export const call: LocalCommandCall = async args => {
  return { type: 'text', value: await runKnowledge(args.trim().split(/\s+/).filter(Boolean)) }
}
