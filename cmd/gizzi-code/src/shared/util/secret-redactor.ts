/**
 * The secret redactor for tool results (AllternitOS plan step 10, CONTRACT.md 2.3).
 *
 * A port of the authority kernel's redactor (`cmd/allternit-api/src/authority/redact.rs`), with
 * the same rules and the same shared test vectors
 * (`spec/Contracts/os/v1/conformance/redaction-vectors.json`). Change the rules in both, with a
 * vector. It masks:
 *
 *  1. the exact values attached to this call (an approved `env_passthrough` credential, a cloud
 *     environment's secrets) and their base64 form, longest first;
 *  2. credential-shaped strings: provider key prefixes (sk-, ghp_, gho_, xox?-, AKIA, AIza,
 *     sk_live_), PEM private key blocks, JWTs, bearer/basic values, passwords in URLs, and long or
 *     high-entropy values assigned to a key named like a credential.
 *
 * `credentialFile` additionally masks every assignment value, for an approved read of a `.env` or
 * credential file. This is separate from `redact.ts` (log lines), which keeps its own format.
 */

export const MASK = "[REDACTED]"

const MIN_VALUE_LEN = 3
const MIN_B64_LEN = 8

// Same patterns as redact.rs. JS has no inline (?i), so case-insensitive rules use the `i` flag.
const WHOLE: RegExp[] = [
  /-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----[\s\S]*?(?:-----END [A-Z0-9 ]*PRIVATE KEY-----|$)/g,
  /\bsk-[A-Za-z0-9][A-Za-z0-9_\-]{19,}/g,
  /\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})/g,
  /\bxox[abposr]-[A-Za-z0-9\-]{10,}/g,
  /\b(?:AKIA|ASIA)[0-9A-Z]{16}\b/g,
  /\bAIza[0-9A-Za-z_\-]{35}/g,
  /\b(?:sk|rk)_(?:live|test)_[0-9A-Za-z]{16,}/g,
  /\beyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}/g,
]
const BEARER = /\b((?:bearer|basic)\s+)([A-Za-z0-9_\-.~+/]{16,}=*)/gi
const URL_CREDS = /\b([A-Za-z][A-Za-z0-9+.\-]*:\/\/[^/\s:@]+:)([^@\s/]{3,})@/g
const ASSIGNMENT =
  /\b([A-Za-z0-9_.\-]*(?:api[_\-]?key|apikey|secret|token|passwd|password|pwd|credential|private[_\-]?key|access[_\-]?key|auth[_\-]?token|authorization)[A-Za-z0-9_.\-]*)(["']?[ \t]*[:=][ \t]*["']?)([^\s"'`,;&<>]+)/gi
const FILE_LINE = /^([ \t]*(?:export[ \t]+)?[A-Za-z_][A-Za-z0-9_.\-]*[ \t]*[=:][ \t]*)(.+)$/gm

function charClasses(v: string): number {
  return [/[a-z]/, /[A-Z]/, /[0-9]/, /[^A-Za-z0-9]/].filter((r) => r.test(v)).length
}

/** Same as `assignment_is_secret` in redact.rs. */
export function assignmentIsSecret(key: string, value: string): boolean {
  const v = value.trim()
  if (v === MASK || v.includes("://") || v.startsWith("/") || v.startsWith("~")) return false
  if (["true", "false", "null", "none", "undefined"].includes(v.toLowerCase())) return false
  if (/^[0-9.]+$/.test(v)) return false
  const k = key.toLowerCase()
  const passwordLike = ["passw", "pwd", "secret", "credential", "private"].some((w) => k.includes(w))
  const len = [...v].length
  return passwordLike ? len >= 6 : len >= 16 && charClasses(v) >= 2
}

/** The credential-shape rules alone. */
export function maskShapes(text: string): string {
  let out = text
  for (const rule of WHOLE) out = out.replace(rule, MASK)
  out = out.replace(BEARER, (_m, lead: string) => `${lead}${MASK}`)
  out = out.replace(URL_CREDS, (_m, lead: string) => `${lead}${MASK}@`)
  out = out.replace(ASSIGNMENT, (m, key: string, sep: string, value: string) =>
    assignmentIsSecret(key, value) ? `${key}${sep}${MASK}` : m,
  )
  return out
}

export function looksLikeSecret(text: string): boolean {
  return maskShapes(text) !== text
}

function base64NoPad(v: string): string {
  return Buffer.from(v, "utf8").toString("base64").replace(/=+$/, "")
}

export class SecretRedactor {
  /** Longest first; plaintext and base64 forms. */
  private readonly values: string[]

  constructor(values: Iterable<string> = []) {
    const out = new Set<string>()
    for (const v of values) {
      if (typeof v !== "string" || [...v].length < MIN_VALUE_LEN) continue
      out.add(v)
      if (Buffer.byteLength(v, "utf8") >= MIN_B64_LEN) out.add(base64NoPad(v))
    }
    this.values = [...out].sort((a, b) => b.length - a.length || (a < b ? -1 : a > b ? 1 : 0))
  }

  /** Mask one string. */
  text(text: string): string {
    if (typeof text !== "string" || text.length === 0) return text
    let out = text
    for (const v of this.values) if (out.includes(v)) out = out.split(v).join(MASK)
    return maskShapes(out)
  }

  /** An approved read of a `.env` or credential file: `text`, then every assignment value. */
  credentialFile(text: string): string {
    return this.text(text).replace(FILE_LINE, (m, lead: string, value: string) => {
      const v = value.trim()
      return v === "" || v === MASK ? m : `${lead}${MASK}`
    })
  }

  /** Mask every string in a JSON-like value (object keys are left alone). */
  value<T>(input: T): T {
    if (typeof input === "string") return this.text(input) as T
    if (Array.isArray(input)) return input.map((v) => this.value(v)) as T
    if (input && typeof input === "object") {
      const out: Record<string, unknown> = {}
      for (const [k, v] of Object.entries(input as Record<string, unknown>)) out[k] = this.value(v)
      return out as T
    }
    return input
  }
}

/**
 * `credentialFile`, line by line, keeping the line count (the read tool numbers lines): a private
 * key block becomes one mask per line, and a bare base64 run (a read that starts inside a key) is
 * masked too.
 */
export function maskCredentialFileLines(lines: string[], redactor = new SecretRedactor()): string[] {
  let inKey = false
  return lines.map((line) => {
    if (/-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----/.test(line)) {
      inKey = !/-----END [A-Z0-9 ]*PRIVATE KEY-----/.test(line)
      return MASK
    }
    if (inKey) {
      if (/-----END [A-Z0-9 ]*PRIVATE KEY-----/.test(line)) inKey = false
      return MASK
    }
    if (/^[ \t]*[A-Za-z0-9+/]{40,}={0,2}[ \t]*$/.test(line)) return MASK
    return redactor.credentialFile(line)
  })
}
