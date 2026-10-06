/**
 * RFC 8785 (JCS) canonical JSON + `sha256:<hex>` hashing, matching the WP3
 * receipt chain in factory/engine/src/workspace/receipts/jcs.rs so hashes computed here are
 * the same material the chain signs.
 */
import { createHash } from "crypto"

export function canonicalize(value: unknown): string {
  if (value === null) return "null"
  switch (typeof value) {
    case "boolean":
      return value ? "true" : "false"
    case "number":
      if (!Number.isFinite(value)) throw new Error("JCS: non-finite number")
      return JSON.stringify(value)
    case "string":
      return JSON.stringify(value)
    case "object": {
      if (Array.isArray(value)) return "[" + value.map((v) => canonicalize(v === undefined ? null : v)).join(",") + "]"
      const obj = value as Record<string, unknown>
      const keys = Object.keys(obj)
        .filter((k) => obj[k] !== undefined)
        .sort()
      return "{" + keys.map((k) => JSON.stringify(k) + ":" + canonicalize(obj[k])).join(",") + "}"
    }
    default:
      throw new Error(`JCS: unsupported type ${typeof value}`)
  }
}

export function sha256Tagged(input: string | Uint8Array): `sha256:${string}` {
  return `sha256:${createHash("sha256").update(input).digest("hex")}`
}

export function hashValue(value: unknown): `sha256:${string}` {
  return sha256Tagged(canonicalize(value))
}

/** Coerce an arbitrary identifier into the ABI `Id` pattern. */
export function toAbiId(raw: string, prefix = "id"): string {
  const cleaned = raw.replace(/[^A-Za-z0-9._:@-]/g, "-").slice(0, 200)
  return /^[A-Za-z0-9]/.test(cleaned) ? cleaned : `${prefix}-${cleaned}`.slice(0, 200)
}

/** Deterministic token estimate (chars/4); logical tokenizer family, never a vendor. */
export const TOKENIZER_FAMILY = "generic.chars4"
export function estimateTokens(text: string): number {
  return Math.ceil(text.length / 4)
}
