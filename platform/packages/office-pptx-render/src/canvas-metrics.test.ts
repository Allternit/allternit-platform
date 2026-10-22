import { afterEach, describe, expect, it, vi } from 'vitest'
import { CanvasMetrics, HeuristicMetrics } from './metrics'

const style = { fontFamily: 'Calibri', fontSizePx: 20, bold: true, italic: false }

describe('CanvasMetrics', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('measures with the renderer font stack through a canvas', () => {
    const fonts: string[] = []
    vi.stubGlobal('OffscreenCanvas', class {
      getContext() {
        return {
          set font(f: string) { fonts.push(f) },
          measureText: (t: string) => ({ width: t.length * 7, fontBoundingBoxAscent: 16, fontBoundingBoxDescent: 4 }),
        }
      }
    })
    const m = new CanvasMetrics((f) => `'${f}', Carlito, sans-serif`)
    expect(m.measure('Rain', style)).toBe(28)
    expect(fonts[0]).toBe("bold 20px 'Calibri', Carlito, sans-serif")
    expect(m.metrics(style)).toEqual({ ascent: 16, descent: 4, lineHeight: 20 })
  })

  it('falls back to the heuristic without a canvas', () => {
    vi.stubGlobal('OffscreenCanvas', undefined)
    vi.stubGlobal('document', undefined)
    const m = new CanvasMetrics()
    expect(m.measure('Rain', style)).toBe(new HeuristicMetrics().measure('Rain', style))
  })

  it('reports fonts measured before they loaded, once, after waiting for them', async () => {
    const load = vi.fn(async () => [])
    let loaded = false
    vi.stubGlobal('document', { fonts: { check: () => loaded, load } })
    vi.stubGlobal('OffscreenCanvas', class {
      getContext() {
        return { font: '', measureText: () => ({ width: loaded ? 10 : 30 }) }
      }
    })
    const m = new CanvasMetrics()
    expect(m.measure('a', style)).toBe(30)
    loaded = true
    expect(await m.settle()).toBe(true)
    expect(load).toHaveBeenCalledTimes(1)
    expect(m.measure('a', style)).toBe(10)
    expect(await m.settle()).toBe(false)
  })
})
