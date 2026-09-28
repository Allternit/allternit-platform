import { useEffect, useState } from 'react'
import { platformApiBase, platformToken } from '@/runtime/bots/platform-api.js'
import { getImageProcessor } from '../tools/FileReadTool/imageProcessor'

/**
 * Image avatars for the pet, as PNG base64 sized for its 12 x 5 cell slot.
 * A cell is about twice as tall as wide, so 12 x 5 cells is a 12:10 box:
 * the avatar is cropped to cover 144 x 120 px and never stretched. Loaded
 * once per URL and kept for the session; null when it can't be loaded, and
 * the pet shows Gizzi instead.
 */
export const AVATAR_COLS = 12
export const AVATAR_ROWS = 5
const WIDTH = 144
const HEIGHT = 120
const MAX_BYTES = 5 * 1024 * 1024
const TIMEOUT_MS = 8000

async function readImage(url: string): Promise<Buffer | null> {
  if (url.startsWith('data:')) {
    const comma = url.indexOf(',')
    if (comma < 0) return null
    const meta = url.slice(5, comma)
    const payload = url.slice(comma + 1)
    return meta.endsWith(';base64') ? Buffer.from(payload, 'base64') : Buffer.from(decodeURIComponent(payload))
  }
  // Images the Allternit platform hosts may need the signed-in account; no
  // other host ever gets the token.
  const headers: Record<string, string> = { Accept: 'image/*' }
  if (new URL(url).origin === new URL(platformApiBase()).origin) {
    const token = await platformToken()
    if (token) headers.Authorization = `Bearer ${token}`
  }
  const response = await fetch(url, { headers, signal: AbortSignal.timeout(TIMEOUT_MS) })
  if (!response.ok) return null
  if (Number(response.headers.get('content-length') ?? 0) > MAX_BYTES) return null
  const bytes = Buffer.from(await response.arrayBuffer())
  return bytes.length > MAX_BYTES ? null : bytes
}

async function load(url: string): Promise<string | null> {
  try {
    const bytes = await readImage(url)
    if (!bytes || bytes.length === 0) return null
    const sharp = await getImageProcessor()
    const png = await sharp(bytes).resize(WIDTH, HEIGHT, { fit: 'cover' }).png().toBuffer()
    return png.toString('base64')
  } catch {
    return null
  }
}

const cache = new Map<string, Promise<string | null>>()

export function loadAvatarPng(url: string): Promise<string | null> {
  let pending = cache.get(url)
  if (!pending) {
    pending = load(url)
    cache.set(url, pending)
  }
  return pending
}

/** Test hook. */
export function clearAvatarCacheForTest(): void {
  cache.clear()
}

/** The avatar PNG for `url` once it has loaded; null before that, on failure, or with no url. */
export function useAvatarPng(url: string | undefined): string | null {
  const [loaded, setLoaded] = useState<{ url: string; png: string | null } | null>(null)
  useEffect(() => {
    if (!url) return
    let live = true
    void loadAvatarPng(url).then(png => {
      if (live) setLoaded({ url, png })
    })
    return () => {
      live = false
    }
  }, [url])
  return url && loaded?.url === url ? loaded.png : null
}
