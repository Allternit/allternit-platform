import { createReadStream, readFileSync, statSync } from "node:fs"
import { createInterface } from "node:readline"

export function readJsonlObjects(path: string, maxLines = 200_000): { records: Record<string, unknown>[]; malformed: number } {
  const st = statSync(path)
  if (st.size === 0) return { records: [], malformed: 0 }
  const text = readFileSync(path, "utf8")
  return parseJsonlText(text, maxLines)
}

export function parseJsonlText(text: string, maxLines = 200_000): { records: Record<string, unknown>[]; malformed: number } {
  const records: Record<string, unknown>[] = []
  let malformed = 0
  const lines = text.split(/\r?\n/)
  const limit = Math.min(lines.length, maxLines)
  for (let i = 0; i < limit; i++) {
    const line = lines[i]!.trim()
    if (!line) continue
    try {
      const value = JSON.parse(line) as unknown
      if (value && typeof value === "object" && !Array.isArray(value)) records.push(value as Record<string, unknown>)
      else malformed++
    } catch {
      malformed++
    }
  }
  return { records, malformed }
}

export async function readJsonlAsync(path: string, maxLines = 200_000): Promise<{ records: Record<string, unknown>[]; malformed: number }> {
  const records: Record<string, unknown>[] = []
  let malformed = 0
  let n = 0
  const rl = createInterface({ input: createReadStream(path, { encoding: "utf8" }), crlfDelay: Infinity })
  for await (const line of rl) {
    const trimmed = line.trim()
    if (!trimmed) continue
    n++
    if (n > maxLines) break
    try {
      const value = JSON.parse(trimmed) as unknown
      if (value && typeof value === "object" && !Array.isArray(value)) records.push(value as Record<string, unknown>)
      else malformed++
    } catch {
      malformed++
    }
  }
  return { records, malformed }
}

export function asString(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined
}

export function asNumber(value: unknown): number | undefined {
  if (typeof value === "number" && Number.isFinite(value)) return value
  if (typeof value === "string" && value) {
    const n = Date.parse(value)
    if (Number.isFinite(n)) return n
  }
  return undefined
}

export function contentText(content: unknown): string {
  if (typeof content === "string") return content
  if (Array.isArray(content)) {
    return content
      .map((item) => {
        if (typeof item === "string") return item
        if (item && typeof item === "object" && "text" in item && typeof (item as { text: unknown }).text === "string") {
          return (item as { text: string }).text
        }
        return ""
      })
      .filter(Boolean)
      .join("\n")
  }
  if (content && typeof content === "object" && "text" in content && typeof (content as { text: unknown }).text === "string") {
    return (content as { text: string }).text
  }
  return ""
}

/** A tool call's arguments as a record: an object as is, a JSON string parsed, anything else empty. */
export function toolInput(value: unknown): Record<string, unknown> {
  if (typeof value === "string") {
    const raw = value
    try {
      value = JSON.parse(raw)
    } catch {
      return raw.trim() ? { input: raw } : {}
    }
  }
  return value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : {}
}
