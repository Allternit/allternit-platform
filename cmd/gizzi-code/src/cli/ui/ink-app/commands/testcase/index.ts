// @ts-nocheck
import type { Command } from '../../commands'

const testcase = {
  type: 'local',
  name: 'testcase',
  description:
    'Save this chat as a test case. Label is good or bad; with no label it saves as bad, because people mostly save what went wrong',
  argumentHint: '[good|bad] [what should have happened]',
  supportsNonInteractive: false,
  load: () => import('./testcase.js'),
} satisfies Command

export default testcase
