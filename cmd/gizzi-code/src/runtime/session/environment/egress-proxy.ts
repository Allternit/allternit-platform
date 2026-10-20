/**
 * Per-session egress proxy for a code environment's network policy.
 *
 * Tool processes of a session whose environment is `limited` or `none` get
 * HTTP_PROXY / HTTPS_PROXY / ALL_PROXY pointing here:
 *   - `limited`: CONNECT tunnels and plain-HTTP requests go through only when
 *     the target host matches the allowlist; everything else gets a 403 with
 *     a message that says which host was refused and why.
 *   - `none`: every request gets a 403.
 *
 * This is a cooperative control: it stops tools that honour proxy variables
 * (curl, git, npm, pip, cargo, most SDKs). A process that opens raw sockets
 * ignores it — the OS sandbox (see SessionSandbox) is what blocks those when
 * the session runs sandboxed. Docs say so.
 *
 * Listens on 127.0.0.1 with an ephemeral port; one per session, closed when
 * the session is deleted or its environment is re-applied.
 */
import * as http from "http"
import * as net from "net"
import { Log } from "@/shared/util/log"

const log = Log.create({ service: "env-egress-proxy" })

export namespace EgressProxy {
  export type Mode = "deny" | "allowlist"
  export type Handle = { port: number; url: string; close(): Promise<void> }

  /**
   * Host patterns:
   *   `example.com`    that host only
   *   `*.example.com`  any subdomain of example.com (not example.com itself)
   *   `.example.com`   example.com and any subdomain
   *   `*`              any host
   */
  export function matches(hostname: string, patterns: string[]): boolean {
    const host = hostname.toLowerCase().replace(/\.$/, "").replace(/^\[|\]$/g, "")
    if (!host) return false
    return patterns.some((raw) => {
      const p = raw.trim().toLowerCase()
      if (!p) return false
      if (p === "*") return true
      if (p.startsWith("*.")) return host.endsWith(p.slice(1))
      if (p.startsWith(".")) return host === p.slice(1) || host.endsWith(p)
      return host === p
    })
  }

  /** Strip scheme/port/path from what people type into an allowlist. */
  export function normalize(patterns: string[]): string[] {
    const out = new Set<string>()
    for (const raw of patterns) {
      let p = (raw ?? "").trim().toLowerCase()
      if (!p) continue
      p = p.replace(/^[a-z][a-z0-9+.-]*:\/\//, "")
      p = p.split("/")[0]!
      if (!p.startsWith("[")) p = p.replace(/:\d+$/, "")
      if (p) out.add(p)
    }
    return [...out]
  }

  function refusal(host: string, mode: Mode): string {
    return mode === "deny"
      ? `Blocked by this environment's network policy: network access is off, so ${host || "this host"} can't be reached.`
      : `Blocked by this environment's network policy: ${host || "(no host)"} is not on the allowed hosts list. Add it to the environment's allowed hosts to reach it.`
  }

  function splitHostPort(value: string, fallbackPort: number): { host: string; port: number } {
    const v = value.trim()
    if (v.startsWith("[")) {
      const end = v.indexOf("]")
      const host = v.slice(1, end)
      const port = Number(v.slice(end + 2)) || fallbackPort
      return { host, port }
    }
    const idx = v.lastIndexOf(":")
    if (idx > 0 && v.indexOf(":") === idx) return { host: v.slice(0, idx), port: Number(v.slice(idx + 1)) || fallbackPort }
    return { host: v, port: fallbackPort }
  }

  export async function start(input: { sessionID: string; mode: Mode; hosts: string[] }): Promise<Handle> {
    const hosts = normalize(input.hosts)
    const allowed = (host: string) => input.mode === "allowlist" && matches(host, hosts)

    const server = http.createServer((req, res) => {
      // A proxy request carries an absolute URL; fall back to the Host header.
      let target: URL | undefined
      try {
        target = new URL(req.url ?? "")
      } catch {
        try {
          target = new URL(`http://${req.headers.host ?? ""}${req.url ?? "/"}`)
        } catch {
          target = undefined
        }
      }
      const host = target?.hostname ?? ""
      if (!target || target.protocol !== "http:" || !allowed(host)) {
        log.info("refused http request", { sessionID: input.sessionID, host })
        res.writeHead(403, { "Content-Type": "text/plain; charset=utf-8", "X-Gizzi-Network-Policy": "refused" })
        res.end(refusal(host, input.mode) + "\n")
        return
      }
      const headers = { ...req.headers }
      delete headers["proxy-connection"]
      delete headers["proxy-authorization"]
      const upstream = http.request(
        {
          host: target.hostname,
          port: target.port ? Number(target.port) : 80,
          path: target.pathname + target.search,
          method: req.method,
          headers,
        },
        (up) => {
          res.writeHead(up.statusCode ?? 502, up.headers)
          up.pipe(res)
        },
      )
      upstream.on("error", (error) => {
        if (!res.headersSent) res.writeHead(502, { "Content-Type": "text/plain; charset=utf-8" })
        res.end(`Upstream error reaching ${host}: ${error.message}\n`)
      })
      req.pipe(upstream)
    })

    server.on("connect", (req, client: net.Socket, head) => {
      const { host, port } = splitHostPort(req.url ?? "", 443)
      client.on("error", () => client.destroy())
      if (!allowed(host)) {
        log.info("refused CONNECT", { sessionID: input.sessionID, host })
        const body = refusal(host, input.mode) + "\n"
        client.end(
          `HTTP/1.1 403 Forbidden\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: ${Buffer.byteLength(body)}\r\nX-Gizzi-Network-Policy: refused\r\n\r\n${body}`,
        )
        return
      }
      const upstream = net.connect(port, host, () => {
        client.write("HTTP/1.1 200 Connection Established\r\n\r\n")
        if (head.length) upstream.write(head)
        upstream.pipe(client)
        client.pipe(upstream)
      })
      upstream.on("error", () => client.destroy())
      client.on("close", () => upstream.destroy())
    })

    server.on("clientError", (_err, socket) => {
      try {
        socket.end("HTTP/1.1 400 Bad Request\r\n\r\n")
      } catch {
        socket.destroy()
      }
    })

    await new Promise<void>((resolve, reject) => {
      server.once("error", reject)
      server.listen(0, "127.0.0.1", () => resolve())
    })
    const address = server.address()
    if (!address || typeof address === "string") {
      server.close()
      throw new Error("Could not start the environment's network proxy")
    }
    log.info("listening", { sessionID: input.sessionID, port: address.port, mode: input.mode, hosts: hosts.length })
    return {
      port: address.port,
      url: `http://127.0.0.1:${address.port}`,
      close: () =>
        new Promise<void>((resolve) => {
          server.close(() => resolve())
          // Drop open tunnels so close() resolves.
          ;(server as any).closeAllConnections?.()
        }),
    }
  }
}
