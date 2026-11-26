import type { PaneRender } from "./index"

export const GIF_MAX_DIMENSION = 640
export const GIF_MAX_BYTES = 8 * 1024 * 1024
export const GIF_MAX_FRAMES = 120

export type GifResult = { image: string; width: number; height: number; frames: number }

/** Counts image descriptors by walking the GIF blocks. Returns undefined if the structure is broken. */
function countFrames(b: Uint8Array): number | undefined {
  let i = 13
  const flags = b[10]!
  if (flags & 0x80) i += 3 * (1 << ((flags & 7) + 1))
  let frames = 0
  const skipSub = (): boolean => {
    while (i < b.length) {
      const n = b[i++]!
      if (n === 0) return true
      i += n
    }
    return false
  }
  while (i < b.length) {
    const t = b[i++]!
    if (t === 0x3b) return i === b.length ? frames : undefined
    if (t === 0x21) {
      i++ // label
      if (!skipSub()) return undefined
    } else if (t === 0x2c) {
      if (i + 9 > b.length) return undefined
      const f = b[i + 8]!
      i += 9
      if (f & 0x80) i += 3 * (1 << ((f & 7) + 1))
      i++ // LZW minimum code size
      if (!skipSub()) return undefined
      frames++
      if (frames > GIF_MAX_FRAMES) return frames
    } else return undefined
  }
  return undefined
}

/** Validates a GIF reply from the app. Undefined means reject it. */
export function gifResult(result: Pick<PaneRender.Result, "image"> | undefined): GifResult | undefined {
  const image = result?.image
  if (typeof image !== "string") return undefined
  const m = /^data:image\/gif;base64,([A-Za-z0-9+/]+={0,2})$/.exec(image)
  if (!m) return undefined
  if (Math.floor((m[1]!.length * 3) / 4) > GIF_MAX_BYTES + 3) return undefined
  let bytes: Uint8Array
  try {
    bytes = new Uint8Array(Buffer.from(m[1]!, "base64"))
  } catch {
    return undefined
  }
  if (bytes.length > GIF_MAX_BYTES || bytes.length < 14) return undefined
  const head = String.fromCharCode(...bytes.subarray(0, 6))
  if (head !== "GIF87a" && head !== "GIF89a") return undefined
  const width = bytes[6]! | (bytes[7]! << 8)
  const height = bytes[8]! | (bytes[9]! << 8)
  if (width < 1 || height < 1 || width > GIF_MAX_DIMENSION || height > GIF_MAX_DIMENSION) return undefined
  if (bytes[bytes.length - 1] !== 0x3b) return undefined
  const frames = countFrames(bytes)
  if (frames === undefined || frames < 1 || frames > GIF_MAX_FRAMES) return undefined
  return { image, width, height, frames }
}
