export type Target = { kind: "port"; port: number } | { kind: "app"; slug: string } | { kind: "invalid" }

/** `3000` is a port; a lowercase slug (letter first, up to 16) is an app. */
export function classifyTarget(raw: string): Target {
  if (/^\d+$/.test(raw)) {
    const port = Number(raw)
    return port >= 1 && port <= 65535 ? { kind: "port", port } : { kind: "invalid" }
  }
  return /^[a-z][a-z0-9]{0,15}$/.test(raw) ? { kind: "app", slug: raw } : { kind: "invalid" }
}
