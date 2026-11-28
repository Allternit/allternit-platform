import { describe, expect, test } from 'bun:test'
import { whatsNewHighlights } from './releaseNotes'

const CL = `# Changelog

## 2.24.0 (2026-11-27)

- **One.** First thing.
- **Two.** \`code\` thing.
- Three
- Four
- Five

## 2.23.0 (2026-11-25)

- **Old.** Older thing.
`

describe('whatsNewHighlights', () => {
  test('first launch on a new version shows up to 4 plain lines', () => {
    const lines = whatsNewHighlights(CL, '2.24.0', '2.23.0')
    expect(lines).toEqual(['One. First thing.', 'Two. code thing.', 'Three', 'Four'])
  })
  test('same version shows nothing', () => {
    expect(whatsNewHighlights(CL, '2.24.0', '2.24.0')).toEqual([])
  })
  test('showWhatsNew false shows nothing', () => {
    expect(whatsNewHighlights(CL, '2.24.0', '2.23.0', false)).toEqual([])
  })
  test('fresh install shows nothing', () => {
    expect(whatsNewHighlights(CL, '2.24.0', undefined)).toEqual([])
  })
})
