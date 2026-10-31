import { randomUUID } from "node:crypto"
import { mkdir, readdir, rm, stat, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"

/**
 * CLI brains take one text prompt, so an image in a message never reached them:
 * they answered "I can't read photo.png". They can open a file, so an image
 * goes to a private temp file and the prompt names its path. The file is
 * removed after a short while, and an old one left by a crash goes on the next
 * write. People send photos of themselves through this.
 */
const KEEP_MS = 10 * 60 * 1000
const STALE_MS = 60 * 60 * 1000

const EXTENSIONS: Record<string, string> = {
  "image/png": "png",
  "image/jpeg": "jpg",
  "image/jpg": "jpg",
  "image/webp": "webp",
  "image/gif": "gif",
}

function imageBytes(data: unknown): Uint8Array | null {
  if (data instanceof Uint8Array) return data
  if (typeof data !== "string") return null
  // A data: URL or bare base64. An http(s) URL is not ours to fetch here.
  const match = data.match(/^data:[^,]*;base64,(.*)$/s)
  if (!match && /^[a-z]+:/i.test(data)) return null
  return new Uint8Array(Buffer.from(match ? match[1] : data, "base64"))
}

/** Image parts of the last user message that has text: the one the CLI is sent. */
function lastUserImages(prompt: any[]): Array<{ bytes: Uint8Array; extension: string }> {
  let found: Array<{ bytes: Uint8Array; extension: string }> = []
  for (const message of prompt) {
    if (message?.role !== "user" || !Array.isArray(message.content)) continue
    if (!message.content.some((p: any) => p?.type === "text" && p.text)) continue
    found = []
    for (const part of message.content) {
      const mediaType = String(part?.mediaType ?? part?.mimeType ?? "").toLowerCase()
      const extension = EXTENSIONS[mediaType]
      if (part?.type !== "file" || !extension) continue
      const bytes = imageBytes(part.data)
      if (bytes?.length) found.push({ bytes, extension })
    }
  }
  return found
}

async function removeStale(dir: string, now: number) {
  for (const name of await readdir(dir).catch(() => [] as string[])) {
    const path = join(dir, name)
    const info = await stat(path).catch(() => null)
    if (info && now - info.mtimeMs > STALE_MS) await rm(path, { force: true }).catch(() => {})
  }
}

/** Saves the message's images and returns the prompt text that names them. */
export async function imageAttachmentNote(prompt: any[], dir = join(tmpdir(), "gizzi-cli-images")): Promise<string> {
  const images = lastUserImages(prompt)
  if (images.length === 0) return ""
  await mkdir(dir, { recursive: true, mode: 0o700 })
  await removeStale(dir, Date.now())
  const paths: string[] = []
  for (const image of images) {
    const path = join(dir, `${randomUUID()}.${image.extension}`)
    await writeFile(path, image.bytes, { mode: 0o600 })
    paths.push(path)
    setTimeout(() => void rm(path, { force: true }).catch(() => {}), KEEP_MS).unref?.()
  }
  return `\n\nAttached image files. Open each one to see it:\n${paths.map((p) => `- ${p}`).join("\n")}`
}
