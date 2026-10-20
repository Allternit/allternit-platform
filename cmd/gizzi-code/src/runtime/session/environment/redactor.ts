/**
 * Secret redaction for cloud-environment sessions.
 *
 * A code environment's secrets reach tool processes (bash, the setup script)
 * as environment variables. Anything those processes print can echo them, so
 * every registered secret value, and its base64 form, is replaced with
 * `[secret:NAME]` before output is stored, sent to the model, or published
 * on the bus.
 *
 * Kept free of other gizzi imports: the bus and the session store call into
 * it, so it must not pull either back in.
 *
 * Values shorter than MIN_LENGTH are not redacted (a one-letter "secret"
 * would shred every word of output); the environment layer warns about them.
 */
export namespace SecretRedactor {
  export const MIN_LENGTH = 4

  type Needle = { value: string; label: string }

  /** sessionID → secret name → value */
  const bySession = new Map<string, Map<string, string>>()
  let needles: Needle[] = []

  function rebuild() {
    const seen = new Map<string, string>()
    for (const secrets of bySession.values()) {
      for (const [name, value] of secrets) {
        if (!value || value.length < MIN_LENGTH) continue
        const label = `[secret:${name}]`
        const forms = new Set<string>([value])
        const b64 = Buffer.from(value, "utf8").toString("base64")
        forms.add(b64)
        forms.add(b64.replace(/=+$/, ""))
        forms.add(Buffer.from(value, "utf8").toString("base64url"))
        for (const form of forms) if (form.length >= MIN_LENGTH && !seen.has(form)) seen.set(form, label)
      }
    }
    // Longest first, so a value that contains another secret is replaced whole.
    needles = [...seen.entries()].map(([value, label]) => ({ value, label })).sort((a, b) => b.value.length - a.value.length)
  }

  /** Register (or replace) one session's secrets. */
  export function register(sessionID: string, secrets: Record<string, string>) {
    const map = new Map<string, string>()
    for (const [name, value] of Object.entries(secrets)) if (typeof value === "string") map.set(name, value)
    if (map.size === 0) bySession.delete(sessionID)
    else bySession.set(sessionID, map)
    rebuild()
  }

  export function unregister(sessionID: string) {
    if (!bySession.delete(sessionID)) return
    rebuild()
  }

  /** True when at least one secret is registered (callers skip work otherwise). */
  export function active(): boolean {
    return needles.length > 0
  }

  export function text(input: string): string {
    if (needles.length === 0 || !input) return input
    let out = input
    for (const { value, label } of needles) {
      if (out.includes(value)) out = out.split(value).join(label)
    }
    return out
  }

  /**
   * Redact every string inside a JSON-like value. Returns the same reference
   * when nothing changed, so untouched payloads are not copied.
   */
  export function deep<T>(value: T, depth = 0): T {
    if (needles.length === 0) return value
    if (typeof value === "string") return text(value) as T
    if (depth > 32 || value === null || typeof value !== "object") return value
    if (value instanceof Uint8Array || value instanceof ArrayBuffer || value instanceof Date) return value
    if (Array.isArray(value)) {
      let changed = false
      const next = value.map((item) => {
        const r = deep(item, depth + 1)
        if (r !== item) changed = true
        return r
      })
      return (changed ? next : value) as T
    }
    let changed = false
    const next: Record<string, unknown> = {}
    for (const [key, item] of Object.entries(value as Record<string, unknown>)) {
      const r = deep(item, depth + 1)
      if (r !== item) changed = true
      next[key] = r
    }
    return (changed ? next : value) as T
  }
}
