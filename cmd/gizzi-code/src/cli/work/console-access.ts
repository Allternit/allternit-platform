/**
 * Computers console, access side: port forward, SSH keys, snippets.
 *
 * Contract additions (not in #2738, recorded in the PR body for lane B):
 *   POST   /api/v1/computers/:id/forward          {remotePort} -> {url, ticket?}  (WebSocket; one connection = one TCP stream, binary frames)
 *   GET    /api/v1/computers/:id/ssh-keys         -> {keys: SshKey[]}
 *   POST   /api/v1/computers/:id/ssh-keys         {label, publicKey} -> SshKey
 *   DELETE /api/v1/computers/:id/ssh-keys/:keyId
 *   GET    /api/v1/computers/snippets             -> {snippets: Snippet[]}
 *   POST   /api/v1/computers/snippets/:id/run     {computers: string[], approvalConfirmed: true} -> {runs: SnippetRun[]}
 * A 404/501 from any of these is "Not available on your account yet." (see console.ts).
 */
import { cloudBase } from "@/cli/work/api"
import { authHeaders, request } from "@/cli/work/console"

export type SshKey = { id: string; label: string; fingerprint?: string; kind?: "computer" | "agent"; createdAt?: string; expiresAt?: string }
export type Snippet = { id: string; name: string; description?: string; command?: string }
export type SnippetRun = { computerId: string; approvalId?: string; changeId?: string; status: string; error?: string }

const json = { "content-type": "application/json" }
const enc = encodeURIComponent

export async function listKeys(id: string): Promise<SshKey[]> {
  const b = (await (await request(`/api/v1/computers/${enc(id)}/ssh-keys`)).json()) as SshKey[] | { keys?: SshKey[] }
  return Array.isArray(b) ? b : (b.keys ?? [])
}
export async function addKey(id: string, label: string, publicKey: string): Promise<SshKey> {
  return (await (await request(`/api/v1/computers/${enc(id)}/ssh-keys`, { method: "POST", headers: json, body: JSON.stringify({ label, publicKey }) })).json()) as SshKey
}
export async function removeKey(id: string, keyId: string): Promise<void> {
  await request(`/api/v1/computers/${enc(id)}/ssh-keys/${enc(keyId)}`, { method: "DELETE" })
}
export async function listSnippets(): Promise<Snippet[]> {
  const b = (await (await request(`/api/v1/computers/snippets`)).json()) as Snippet[] | { snippets?: Snippet[] }
  return Array.isArray(b) ? b : (b.snippets ?? [])
}
export async function runSnippet(snippetId: string, computers: string[]): Promise<SnippetRun[]> {
  const b = (await (
    await request(`/api/v1/computers/snippets/${enc(snippetId)}/run`, { method: "POST", headers: json, body: JSON.stringify({ computers, approvalConfirmed: true }) })
  ).json()) as SnippetRun[] | { runs?: SnippetRun[] }
  return Array.isArray(b) ? b : (b.runs ?? [])
}

// ── forward ───────────────────────────────────────────────────────────────

export function parsePort(v: unknown, name: string): number {
  const n = typeof v === "number" ? v : /^\d+$/.test(String(v ?? "").trim()) ? Number(v) : NaN
  if (!Number.isInteger(n) || n < 1 || n > 65535) throw new Error(`${name} must be a port number from 1 to 65535.`)
  return n
}

export function parseList(v: unknown): string[] {
  return String(v ?? "")
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean)
    .filter((s, i, a) => a.indexOf(s) === i)
}

/** `ssh-ed25519 AAAA... comment` -> checked; returns the trimmed key. */
export function checkPublicKey(text: string): string {
  const t = text.trim()
  if (/PRIVATE KEY/.test(t)) throw new Error("That is a private key. Give the public key (the .pub file).")
  if (!/^(ssh-(ed25519|rsa)|ecdsa-sha2-nistp\d+|sk-ssh-ed25519@openssh\.com) [A-Za-z0-9+/=]+( .*)?$/.test(t)) throw new Error("That does not look like an SSH public key.")
  return t
}

export type ForwardGrant = { url: string; ticket?: string }

export async function openForward(id: string, remotePort: number): Promise<ForwardGrant> {
  return (await (await request(`/api/v1/computers/${enc(id)}/forward`, { method: "POST", headers: json, body: JSON.stringify({ remotePort }) })).json()) as ForwardGrant
}

/** Resolve a grant url (absolute ws(s) or a path on the cloud base) into a WebSocket url. */
export function forwardSocketUrl(grant: ForwardGrant, base = cloudBase()): string {
  const u = new URL(grant.url, base)
  if (u.protocol === "http:") u.protocol = "ws:"
  if (u.protocol === "https:") u.protocol = "wss:"
  if (grant.ticket) u.searchParams.set("ticket", grant.ticket)
  return u.toString()
}

type Conn = { ws?: WebSocket; pending: Uint8Array<ArrayBuffer>[] }

/**
 * Listen on 127.0.0.1:localPort; each local connection mints a grant and
 * pipes bytes over one WebSocket. Resolves when `signal` aborts.
 */
export async function runForward(id: string, remotePort: number, localPort: number, signal: AbortSignal, log: (s: string) => void): Promise<void> {
  const headers = await authHeaders()
  const server = Bun.listen<Conn>({
    hostname: "127.0.0.1",
    port: localPort,
    socket: {
      open(sock) {
        sock.data = { pending: [] }
        openForward(id, remotePort)
          .then((grant) => {
            const ws = new WebSocket(forwardSocketUrl(grant), { headers } as never)
            ws.binaryType = "arraybuffer"
            ws.onopen = () => {
              sock.data.pending.forEach((c) => ws.send(c))
              sock.data.pending = []
              sock.data.ws = ws
            }
            ws.onmessage = (e) => sock.write(typeof e.data === "string" ? e.data : new Uint8Array(e.data as ArrayBuffer))
            ws.onclose = () => sock.end()
            ws.onerror = () => sock.end()
          })
          .catch((e) => {
            log(`Connection refused: ${(e as Error).message}`)
            sock.end()
          })
      },
      data(sock, chunk) {
        if (sock.data.ws) sock.data.ws.send(new Uint8Array(chunk))
        else sock.data.pending.push(new Uint8Array(chunk))
      },
      close(sock) {
        sock.data?.ws?.close()
      },
      error() {},
    },
  })
  await new Promise<void>((r) => (signal.aborted ? r() : signal.addEventListener("abort", () => r(), { once: true })))
  server.stop(true)
}

// ── formatting ────────────────────────────────────────────────────────────

const pad = (s: string, n: number) => (s.length >= n ? s.slice(0, n - 1) + "…" : s.padEnd(n))

export function formatKeys(rows: SshKey[]): string {
  if (!rows.length) return "No SSH keys on this computer."
  return [`${pad("ID", 14)} ${pad("LABEL", 24)} ${pad("KIND", 9)} ${pad("EXPIRES", 22)} FINGERPRINT`]
    .concat(rows.map((k) => `${pad(k.id, 14)} ${pad(k.label, 24)} ${pad(k.kind ?? "computer", 9)} ${pad(k.expiresAt ?? "never", 22)} ${k.fingerprint ?? "-"}`))
    .join("\n")
}

export function formatSnippets(rows: Snippet[]): string {
  if (!rows.length) return "No saved snippets."
  return [`${pad("ID", 14)} ${pad("NAME", 28)} DESCRIPTION`].concat(rows.map((s) => `${pad(s.id, 14)} ${pad(s.name, 28)} ${s.description ?? ""}`)).join("\n")
}

export function formatRuns(rows: SnippetRun[], names: Record<string, string> = {}): string {
  if (!rows.length) return "No runs were started."
  return rows
    .map((r) => `${pad(names[r.computerId] ?? r.computerId, 28)} ${pad(r.status, 12)} ${r.error ?? (r.approvalId ? `approval ${r.approvalId}` : "")}${r.changeId ? ` change ${r.changeId}` : ""}`.trimEnd())
    .join("\n")
}
