import { describe, expect, test } from "bun:test"
import { mkdtemp, readFile, stat } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { imageAttachmentNote } from "./cli-image-attachments"

const png = new Uint8Array([137, 80, 78, 71, 1, 2, 3])
const base64 = Buffer.from(png).toString("base64")
const user = (...content: unknown[]) => ({ role: "user", content })
const text = { type: "text", text: "draw this" }
const dir = () => mkdtemp(join(tmpdir(), "cli-images-"))
const pathIn = (note: string) => note.split("- ")[1].trim()

describe("imageAttachmentNote", () => {
  test("writes an image from bytes and names the private file", async () => {
    const note = await imageAttachmentNote([user(text, { type: "file", mediaType: "image/png", data: png })], await dir())
    const path = pathIn(note)
    expect(path.endsWith(".png")).toBe(true)
    expect(new Uint8Array(await readFile(path))).toEqual(png)
    expect((await stat(path)).mode & 0o777).toBe(0o600)
  })

  test("reads a data URL and bare base64", async () => {
    for (const data of [`data:image/jpeg;base64,${base64}`, base64]) {
      const note = await imageAttachmentNote([user(text, { type: "file", mediaType: "image/jpeg", data })], await dir())
      expect(new Uint8Array(await readFile(pathIn(note)))).toEqual(png)
    }
  })

  test("uses only the last message with text", async () => {
    const old = user(text, { type: "file", mediaType: "image/png", data: png })
    expect(await imageAttachmentNote([old, user(text)], await dir())).toBe("")
  })

  test("ignores other files, remote URLs and messages with no image", async () => {
    const parts = [
      { type: "file", mediaType: "application/pdf", data: base64 },
      { type: "file", mediaType: "image/png", data: "https://example.com/a.png" },
    ]
    expect(await imageAttachmentNote([user(text, ...parts)], await dir())).toBe("")
    expect(await imageAttachmentNote([{ role: "user", content: "plain" }], await dir())).toBe("")
  })
})
