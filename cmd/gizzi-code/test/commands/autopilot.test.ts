import { describe, expect, test } from 'bun:test'
import {
  digestIsEmpty,
  formatAutopilot,
  formatDigest,
  formatRule,
} from '../../src/runtime/bots/floor-autopilot'
import { isCronShape } from '../../src/runtime/tools/builtins/suggest-task'

describe('autopilot rule formatting', () => {
  test('levels read in plain words', () => {
    const base = { id: 'r', scope: { type: 'all' as const }, need: 'suggestion' as const }
    expect(formatRule({ ...base, level: 'draft' })).toContain('Draft only')
    expect(formatRule({ ...base, level: 'ask' })).toContain('Ask first')
    expect(formatRule({ ...base, level: 'tell' })).toContain('Send and tell me')
    expect(formatRule({ ...base, level: 'limits', limits: { maxPerDay: 5 } })).toContain(
      'Act within limits (up to 5 a day)',
    )
  })

  test('questions always show Draft only', () => {
    const line = formatRule({ id: 'q', scope: { type: 'all' }, need: 'question', level: 'tell' })
    expect(line).toContain('Draft only')
  })

  test('status shows on/off, main bot and empty rules', () => {
    const text = formatAutopilot({ enabled: false, mainBotName: 'Gizzi', rules: [] })
    expect(text).toContain('Autopilot is off.')
    expect(text).toContain('Main bot: Gizzi')
    expect(text).toContain('No rules yet')
  })
})

describe('away digest rendering', () => {
  const empty = { since: 1, until: 2, answered: [], waiting: [], finished: [], suggestions: [] }

  test('empty digest', () => {
    expect(digestIsEmpty(empty)).toBe(true)
    expect(formatDigest(empty)).toBe('Nothing happened while you were away.')
  })

  test('non-empty digest marks autopilot answers', () => {
    const text = formatDigest({
      ...empty,
      answered: [{ botId: 'b', text: 'Approved the read', at: 1, ruleId: 'r1' }],
      waiting: [{ botId: 'b', text: 'Send the invoice?', at: 1 }],
    })
    expect(text).toContain('While you were away')
    expect(text).toContain('Approved the read (on autopilot)')
    expect(text).toContain('Waiting for you (1)')
    expect(text).not.toContain('Finished')
  })
})

describe('suggest_task schedule cron shape', () => {
  test('accepts five fields, rejects others', () => {
    expect(isCronShape('0 9 * * 1')).toBe(true)
    expect(isCronShape('*/15 * * * *')).toBe(true)
    expect(isCronShape('every monday')).toBe(false)
    expect(isCronShape('0 9 * *')).toBe(false)
  })
})
