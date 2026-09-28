import z from "zod/v4"
import { createReadStream } from "fs"
import * as fs from "fs/promises"
import * as path from "path"
import { createInterface } from "readline"
import { Tool } from "@/runtime/tools/builtins/tool"
import { LSP } from "@/runtime/integrations/lsp"
import { FileTime } from "@/shared/file/time"
import DESCRIPTION from "@/runtime/tools/builtins/read.txt"
import { Instance } from "@/runtime/context/project/instance"
import { assertExternalDirectory } from "@/runtime/tools/builtins/external-directory"
import { InstructionPrompt } from "@/runtime/session/instruction"
import { Filesystem } from "@/shared/util/filesystem"

const DEFAULT_READ_LIMIT = 2000
const MAX_LINE_LENGTH = 2000
const MAX_LINE_SUFFIX = `... (line truncated to ${MAX_LINE_LENGTH} chars)`
const MAX_BYTES = 50 * 1024
const MAX_BYTES_LABEL = `${MAX_BYTES / 1024} KB`
const MAX_MEDIA_BYTES = 100 * 1024 * 1024
const MAX_PDF_BYTES = 20 * 1024 * 1024
const IMAGE_BYTE_BUDGET = 5 * 1024 * 1024
const IMAGE_MAX_EDGE = 1568

export const ReadTool = Tool.define("read", {
  description: DESCRIPTION,
  parameters: z.object({
    filePath: z.string().describe("The absolute path to the file or directory to read"),
    offset: z.coerce.number().describe("The line number to start reading from (1-indexed)").optional(),
    limit: z.coerce.number().describe("The maximum number of lines to read (defaults to 2000)").optional(),
    region: z.object({
      x: z.number().int().min(0),
      y: z.number().int().min(0),
      width: z.number().int().min(1),
      height: z.number().int().min(1),
    }).optional().describe("Images only: crop in original-image pixel coordinates for fine-detail inspection."),
    fullResolution: z.boolean().optional().describe("Images only: skip downscaling; fails if the native payload exceeds the safe byte budget."),
  }),
  async execute(params, ctx) {
    if (params.offset !== undefined && params.offset < 1) {
      throw new Error("offset must be greater than or equal to 1")
    }
    let filepath = params.filePath
    if (!path.isAbsolute(filepath)) {
      filepath = path.resolve(Instance.workdir, filepath)
    }
    const title = path.relative(Instance.worktree, filepath)

    const stat = Filesystem.stat(filepath)

    await assertExternalDirectory(ctx, filepath, {
      bypass: Boolean(ctx.extra?.["bypassCwdCheck"]),
      kind: stat?.isDirectory() ? "directory" : "file",
    })

    await ctx.ask({
      permission: "read",
      patterns: [filepath],
      always: ["*"],
      metadata: {},
    })

    if (!stat) {
      const dir = path.dirname(filepath)
      const base = path.basename(filepath)

      const suggestions = await fs
        .readdir(dir)
        .then((entries) =>
          entries
            .filter(
              (entry) =>
                entry.toLowerCase().includes(base.toLowerCase()) || base.toLowerCase().includes(entry.toLowerCase()),
            )
            .map((entry) => path.join(dir, entry))
            .slice(0, 3),
        )
        .catch(() => [])

      if (suggestions.length > 0) {
        throw new Error(`File not found: ${filepath}\n\nDid you mean one of these?\n${suggestions.join("\n")}`)
      }

      throw new Error(`File not found: ${filepath}`)
    }

    if (stat.isDirectory()) {
      const dirents = await fs.readdir(filepath, { withFileTypes: true })
      const entries = await Promise.all(
        dirents.map(async (dirent) => {
          if (dirent.isDirectory()) return dirent.name + "/"
          if (dirent.isSymbolicLink()) {
            const target = await fs.stat(path.join(filepath, dirent.name)).catch(() => undefined)
            if (target?.isDirectory()) return dirent.name + "/"
          }
          return dirent.name
        }),
      )
      entries.sort((a, b) => a.localeCompare(b))

      const limit = params.limit ?? DEFAULT_READ_LIMIT
      const offset = params.offset ?? 1
      const start = offset - 1
      const sliced = entries.slice(start, start + limit)
      const truncated = start + sliced.length < entries.length

      const output = [
        `<path>${filepath}</path>`,
        `<type>directory</type>`,
        `<entries>`,
        sliced.join("\n"),
        truncated
          ? `\n(Showing ${sliced.length} of ${entries.length} entries. Use 'offset' parameter to read beyond entry ${offset + sliced.length})`
          : `\n(${entries.length} entries)`,
        `</entries>`,
      ].join("\n")

      return {
        title,
        output,
        metadata: {
          preview: sliced.slice(0, 20).join("\n"),
          truncated,
          loaded: [] as string[],
        },
      }
    }

    const instructions = await InstructionPrompt.resolve(ctx.messages, filepath, ctx.messageID)

    // Exclude SVG (XML-based) and vnd.fastbidsheet (.fbs extension, commonly FlatBuffers schema files)
    const mime = Filesystem.mimeType(filepath)
    const isImage = mime.startsWith("image/") && mime !== "image/svg+xml" && mime !== "image/vnd.fastbidsheet"
    const isPdf = mime === "application/pdf"
    if (isImage || isPdf) {
      if (isPdf && Number(stat.size) > MAX_PDF_BYTES) {
        throw new Error(`PDF is ${stat.size} bytes; the safe inline limit is ${MAX_PDF_BYTES} bytes. Extract or split the needed pages first.`)
      }
      if (isPdf && (params.region || params.fullResolution)) throw new Error("region and fullResolution are only valid for images")
      const prepared = isImage
        ? await prepareImage(filepath, { region: params.region, fullResolution: params.fullResolution === true })
        : { bytes: await Filesystem.readBytes(filepath), mime, note: "PDF read successfully." }
      return {
        title,
        output: prepared.note,
        metadata: {
          preview: prepared.note,
          truncated: false,
          loaded: instructions.map((i) => i.filepath),
        },
        attachments: [
          {
            type: "file",
            mime: prepared.mime,
            url: `data:${prepared.mime};base64,${prepared.bytes.toString("base64")}`,
          },
        ],
      }
    }

    const isBinary = await isBinaryFile(filepath, Number(stat.size))
    if (isBinary) throw new Error(`Cannot read binary file: ${filepath}`)

    const stream = createReadStream(filepath, { encoding: "utf8" })
    const rl = createInterface({
      input: stream,
      // Note: we use the crlfDelay option to recognize all instances of CR LF
      // ('\r\n') in file as a single line break.
      crlfDelay: Infinity,
    })

    const limit = params.limit ?? DEFAULT_READ_LIMIT
    const offset = params.offset ?? 1
    const start = offset - 1
    const raw: string[] = []
    let bytes = 0
    let lines = 0
    let truncatedByBytes = false
    let hasMoreLines = false
    try {
      for await (const text of rl) {
        lines += 1
        if (lines <= start) continue

        if (raw.length >= limit) {
          hasMoreLines = true
          continue
        }

        const line = text.length > MAX_LINE_LENGTH ? text.substring(0, MAX_LINE_LENGTH) + MAX_LINE_SUFFIX : text
        const size = Buffer.byteLength(line, "utf-8") + (raw.length > 0 ? 1 : 0)
        if (bytes + size > MAX_BYTES) {
          truncatedByBytes = true
          hasMoreLines = true
          break
        }

        raw.push(line)
        bytes += size
      }
    } finally {
      rl.close()
      stream.destroy()
    }

    if (lines < offset && !(lines === 0 && offset === 1)) {
      throw new Error(`Offset ${offset} is out of range for this file (${lines} lines)`)
    }

    const content = raw.map((line, index) => {
      return `${index + offset}: ${line}`
    })
    const preview = raw.slice(0, 20).join("\n")

    let output = [`<path>${filepath}</path>`, `<type>file</type>`, "<content>"].join("\n")
    output += content.join("\n")

    const totalLines = lines
    const lastReadLine = offset + raw.length - 1
    const nextOffset = lastReadLine + 1
    const truncated = hasMoreLines || truncatedByBytes

    if (truncatedByBytes) {
      output += `\n\n(Output capped at ${MAX_BYTES_LABEL}. Showing lines ${offset}-${lastReadLine}. Use offset=${nextOffset} to continue.)`
    } else if (hasMoreLines) {
      output += `\n\n(Showing lines ${offset}-${lastReadLine} of ${totalLines}. Use offset=${nextOffset} to continue.)`
    } else {
      output += `\n\n(End of file - total ${totalLines} lines)`
    }
    output += "\n</content>"

    // just warms the lsp client
    LSP.touchFile(filepath, false)
    FileTime.read(ctx.sessionID, filepath)

    if (instructions.length > 0) {
      output += `\n\n<system-reminder>\n${instructions.map((i) => i.content).join("\n\n")}\n</system-reminder>`
    }

    return {
      title,
      output,
      metadata: {
        preview,
        truncated,
        loaded: instructions.map((i) => i.filepath),
      },
    }
  },
})

async function prepareImage(filepath: string, options: {
  region?: { x: number; y: number; width: number; height: number }
  fullResolution: boolean
}) {
  const original = Buffer.from(await Filesystem.readBytes(filepath))
  if (!original.length) throw new Error(`Image file is empty: ${filepath}`)
  if (original.length > MAX_MEDIA_BYTES) {
    throw new Error(`Image is ${original.length} bytes; the safe decode limit is ${MAX_MEDIA_BYTES} bytes. Create a smaller copy first.`)
  }
  const imported = await import("sharp")
  const sharp = (imported.default ?? imported) as typeof imported.default
  const source = sharp(original, { animated: false }).rotate()
  const metadata = await source.metadata()
  const width = metadata.width ?? 0
  const height = metadata.height ?? 0
  if (!width || !height) throw new Error(`Could not determine image dimensions: ${filepath}`)

  const accepted = new Set(["image/png", "image/jpeg", "image/gif", "image/webp"])
  const originalMime = normalizeImageMime(metadata.format ? `image/${metadata.format}` : Filesystem.mimeType(filepath))
  if (options.fullResolution && options.region) throw new Error("Use either region or fullResolution, not both")
  if (options.fullResolution && original.length > IMAGE_BYTE_BUDGET) {
    throw new Error(`fullResolution cannot be honored: ${original.length} bytes exceeds the ${IMAGE_BYTE_BUDGET}-byte limit. Use region instead.`)
  }

  let pipeline = sharp(original, { animated: false }).rotate()
  let delivery = "native resolution"
  if (options.region) {
    const { x, y, width: cropWidth, height: cropHeight } = options.region
    if (x + cropWidth > width || y + cropHeight > height) {
      throw new Error(`Region (${x},${y},${cropWidth},${cropHeight}) exceeds original image bounds ${width}x${height}`)
    }
    pipeline = pipeline.extract({ left: x, top: y, width: cropWidth, height: cropHeight })
    delivery = `region x=${x}, y=${y}, width=${cropWidth}, height=${cropHeight} in original-image pixels`
  } else if (!options.fullResolution && Math.max(width, height) > IMAGE_MAX_EDGE) {
    pipeline = pipeline.resize({ width: IMAGE_MAX_EDGE, height: IMAGE_MAX_EDGE, fit: "inside", withoutEnlargement: true })
    delivery = `downsampled to a maximum ${IMAGE_MAX_EDGE}px edge`
  }

  const shouldPreserve = !options.region &&
    (options.fullResolution || Math.max(width, height) <= IMAGE_MAX_EDGE) &&
    accepted.has(originalMime) && original.length <= IMAGE_BYTE_BUDGET
  if (shouldPreserve) {
    return {
      bytes: original,
      mime: originalMime,
      note: `Image read successfully. Original ${width}x${height}, ${original.length} bytes, delivered at ${delivery}.`,
    }
  }

  const hasAlpha = metadata.hasAlpha === true
  let output = hasAlpha
    ? await pipeline.clone().png({ compressionLevel: 9 }).toBuffer()
    : await pipeline.clone().jpeg({ quality: 82, mozjpeg: true }).toBuffer()
  let outputMime = hasAlpha ? "image/png" : "image/jpeg"
  if (output.length > IMAGE_BYTE_BUDGET) {
    for (const quality of [70, 50, 30]) {
      output = await pipeline.clone().jpeg({ quality, mozjpeg: true }).toBuffer()
      outputMime = "image/jpeg"
      if (output.length <= IMAGE_BYTE_BUDGET) break
    }
  }
  if (output.length > IMAGE_BYTE_BUDGET) {
    throw new Error(`Image remains ${output.length} bytes after compression; limit is ${IMAGE_BYTE_BUDGET}. Use a smaller region or create a smaller copy.`)
  }
  const outputMeta = await sharp(output).metadata()
  const conversion = accepted.has(originalMime) ? "" : ` Converted unsupported ${originalMime} to ${outputMime}.`
  return {
    bytes: output,
    mime: outputMime,
    note: `Image read successfully. Original ${width}x${height}, ${original.length} bytes; delivered ${outputMeta.width}x${outputMeta.height}, ${output.length} bytes (${delivery}).${conversion}`,
  }
}

function normalizeImageMime(mime: string) {
  const base = mime.toLowerCase().split(";", 1)[0]!.trim()
  return base === "image/jpg" ? "image/jpeg" : base
}

async function isBinaryFile(filepath: string, fileSize: number): Promise<boolean> {
  const ext = path.extname(filepath).toLowerCase()
  // binary check for common non-text extensions
  switch (ext) {
    case ".zip":
    case ".tar":
    case ".gz":
    case ".exe":
    case ".dll":
    case ".so":
    case ".class":
    case ".jar":
    case ".war":
    case ".7z":
    case ".doc":
    case ".docx":
    case ".xls":
    case ".xlsx":
    case ".ppt":
    case ".pptx":
    case ".odt":
    case ".ods":
    case ".odp":
    case ".bin":
    case ".dat":
    case ".obj":
    case ".o":
    case ".a":
    case ".lib":
    case ".wasm":
    case ".pyc":
    case ".pyo":
      return true
    default:
      break
  }

  if (fileSize === 0) return false

  const fh = await fs.open(filepath, "r")
  try {
    const sampleSize = Math.min(4096, fileSize)
    const bytes = Buffer.alloc(sampleSize)
    const result = await fh.read(bytes, 0, sampleSize, 0)
    if (result.bytesRead === 0) return false

    let nonPrintableCount = 0
    for (let i = 0; i < result.bytesRead; i++) {
      if (bytes[i] === 0) return true
      if (bytes[i] < 9 || (bytes[i] > 13 && bytes[i] < 32)) {
        nonPrintableCount++
      }
    }
    // If >30% non-printable characters, consider it binary
    return nonPrintableCount / result.bytesRead > 0.3
  } finally {
    await fh.close()
  }
}
