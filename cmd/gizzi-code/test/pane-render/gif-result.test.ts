import { describe, expect, test } from "bun:test"
import { gifResult } from "../../src/runtime/integrations/pane-render/gif-result"

function gif(frames: number, w = 2, h = 2, trailer = true) {
  const out: number[] = [...Buffer.from("GIF89a"), w & 255, w >> 8, h & 255, h >> 8, 0x80, 0, 0, 0, 0, 0, 255, 255, 255]
  for (let f = 0; f < frames; f++) {
    out.push(0x21, 0xf9, 4, 0, 10, 0, 0, 0) // graphic control extension
    out.push(0x2c, 0, 0, 0, 0, w & 255, w >> 8, h & 255, h >> 8, 0, 2, 2, 0x44, 0x01, 0)
  }
  if (trailer) out.push(0x3b)
  return "data:image/gif;base64," + Buffer.from(out).toString("base64")
}

describe("gifResult", () => {
  test("accepts a valid 2-frame gif", () => {
    const r = gifResult({ image: gif(2) })
    expect(r).toMatchObject({ width: 2, height: 2, frames: 2 })
  })
  test("rejects a truncated gif", () => {
    const full = gif(2)
    const raw = Buffer.from(full.split(",")[1]!, "base64").subarray(0, 30)
    expect(gifResult({ image: "data:image/gif;base64," + raw.toString("base64") })).toBeUndefined()
    expect(gifResult({ image: gif(2, 2, 2, false) })).toBeUndefined()
  })
  test("rejects oversize dimensions and zero frames", () => {
    expect(gifResult({ image: gif(1, 641, 10) })).toBeUndefined()
    expect(gifResult({ image: gif(0) })).toBeUndefined()
    expect(gifResult({ image: gif(121) })).toBeUndefined()
  })
  test("rejects a PNG passed as a gif", () => {
    const png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/l9sAAAAASUVORK5CYII="
    expect(gifResult({ image: "data:image/gif;base64," + png })).toBeUndefined()
    expect(gifResult({ image: "data:image/png;base64," + png })).toBeUndefined()
  })
})
