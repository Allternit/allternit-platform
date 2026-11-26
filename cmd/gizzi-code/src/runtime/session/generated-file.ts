/**
 * Files a model generates (AI SDK `file` stream parts) become FileParts on the
 * assistant message, for every provider: an image from a Gemini image model,
 * or a Subscription Fabric artifact (image, deck, document) downloaded from
 * the user's Sessions computer.
 *
 * A provider can precede a `file` part with a raw `__gizzi: "generated_file"`
 * part carrying what the AI SDK file part cannot: its filename, a title and
 * where it came from. A raw part with a `url` and no bytes is a file too large
 * to inline — it becomes a FilePart pointing at that URL.
 */

import type { MessageV2 } from "./message-v2"

export interface GeneratedFileMeta {
  filename?: string
  title?: string
  mediaType?: string
  /** Where the file came from (e.g. `fabric-artifact://<id>`). */
  sourceUri?: string
  /** Set when the bytes were not inlined: the file is fetched from here. */
  url?: string
}

export function generatedFileMeta(raw: Record<string, unknown>): GeneratedFileMeta | undefined {
  if (raw.__gizzi !== "generated_file") return undefined
  const str = (v: unknown) => (typeof v === "string" && v.trim() ? v : undefined)
  return {
    filename: str(raw.filename),
    title: str(raw.title),
    mediaType: str(raw.mediaType),
    sourceUri: str(raw.sourceUri),
    url: str(raw.url),
  }
}

export function generatedFilePart(input: {
  id: string
  messageID: string
  sessionID: string
  mediaType?: string
  /** base64 bytes; absent when meta.url points at the file instead */
  base64?: string
  meta?: GeneratedFileMeta
}): MessageV2.FilePart | undefined {
  const mime = input.mediaType || input.meta?.mediaType || "application/octet-stream"
  const url = input.base64 !== undefined ? `data:${mime};base64,${input.base64}` : input.meta?.url
  if (!url) return undefined
  const part: MessageV2.FilePart = {
    id: input.id,
    messageID: input.messageID,
    sessionID: input.sessionID,
    type: "file",
    mime,
    url,
  }
  if (input.meta?.filename) part.filename = input.meta.filename
  if (input.meta?.sourceUri) {
    const title = input.meta.title ?? input.meta.filename ?? ""
    part.source = {
      type: "resource",
      clientName: "generated",
      uri: input.meta.sourceUri,
      text: { value: title, start: 0, end: title.length },
    }
  }
  return part
}

/** Source uri of a file a tool call produced: `tool-call://<callID>`. Clients use it to put the file on that call's card. */
export const TOOL_CALL_URI = "tool-call://"
/** Largest picture a tool result publishes as a generated file (decoded bytes). */
export const MAX_TOOL_OUTPUT_FILE_BYTES = 8 * 1024 * 1024

/**
 * The generated-file part for a tool result whose metadata sets `outputFile: true`
 * (media_generate's engine pictures and GIFs): its first PNG or GIF attachment, so
 * the picture rides the generated-artifact path every client already renders (live
 * `artifact` frame, reload `metadata.parts`). The attachment stays on the tool
 * result for the model; assistant file parts never go to the model.
 */
export function toolOutputFilePart(input: {
  id: string
  messageID: string
  sessionID: string
  callID: string
  metadata?: Record<string, unknown>
  attachments?: ReadonlyArray<{ mime: string; url: string; filename?: string }>
}): MessageV2.FilePart | undefined {
  if (input.metadata?.outputFile !== true || !input.callID) return undefined
  const file = input.attachments?.find(
    (a) => (a.mime === "image/png" || a.mime === "image/gif") && a.url.startsWith(`data:${a.mime};base64,`),
  )
  if (!file) return undefined
  const base64 = file.url.slice(`data:${file.mime};base64,`.length)
  if (!base64 || Math.floor((base64.length * 3) / 4) > MAX_TOOL_OUTPUT_FILE_BYTES) return undefined
  return generatedFilePart({
    id: input.id,
    messageID: input.messageID,
    sessionID: input.sessionID,
    mediaType: file.mime,
    base64,
    meta: { filename: file.filename, title: file.filename, sourceUri: `${TOOL_CALL_URI}${input.callID}` },
  })
}
