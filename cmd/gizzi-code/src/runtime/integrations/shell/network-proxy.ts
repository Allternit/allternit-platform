/**
 * Sandbox Network Proxy
 *
 * A minimal CONNECT-tunneling HTTP proxy giving the sandbox domain-allowlisted
 * network access instead of an all-or-nothing block/allow. No TLS termination
 * — CONNECT requests are tunneled raw, matching gizzi-code's default
 * (non-inspecting) behavior; only the CONNECT target hostname / plain-HTTP
 * Host header is checked against the allowlist.
 *
 * Platform wiring lives in sandbox.ts:
 *   - macOS (Seatbelt): the sandboxed process shares the host's loopback, so
 *     this listens on a TCP port and Seatbelt allows outbound only to it.
 *   - Linux (bwrap): `--unshare-net` gives the sandbox its own private
 *     loopback, disconnected from the host's — a host TCP proxy is
 *     unreachable from inside. This listens on a unix socket instead (a
 *     filesystem object, unaffected by network-namespace isolation), which
 *     sandbox.ts bind-mounts in and bridges to a TCP port *inside* the
 *     sandbox's own netns via `socat`, so ordinary HTTP_PROXY-aware tools work.
 */
import * as http from "http"
import * as net from "net"
import * as os from "os"
import * as path from "path"
import { unlinkSync } from "fs"
import { Log } from "@/shared/util/log"

const log = Log.create({ service: "sandbox-network-proxy" })

export interface NetworkProxyHandle {
  mode: "tcp" | "unix"
  port?: number
  socketPath?: string
  close(): Promise<void>
}

export interface NetworkProxyOptions {
  /**
   * Exact-match mode (self-hosted environments, `network: limited`): a host
   * is allowed only when it equals an entry; no subdomains. An entry may
   * carry a port (`git.example.com:8443`), which then must match too.
   * Refusals carry the code `host_not_allowed`.
   */
  exact?: boolean
}

/** Exported for tests. */
export function hostAllowed(hostname: string, allowedDomains: string[], opts: NetworkProxyOptions = {}, port?: number): boolean {
  const host = hostname.toLowerCase().replace(/\.$/, "")
  return allowedDomains.some((domain) => {
    const d = domain.trim().toLowerCase()
    if (!d) return false
    if (opts.exact) {
      const idx = d.lastIndexOf(":")
      const hasPort = idx > 0 && !d.startsWith("[") && /^\d+$/.test(d.slice(idx + 1))
      if (hasPort) return host === d.slice(0, idx) && port === Number(d.slice(idx + 1))
      return host === d
    }
    return host === d || host.endsWith("." + d)
  })
}

function refusalText(hostname: string, opts: NetworkProxyOptions): string {
  return opts.exact
    ? `host_not_allowed: ${hostname || "(no host)"} is not on this environment's allowed_hosts list`
    : `Blocked by sandbox network policy: ${hostname || "(no host)"} is not in the allowlist`
}

function createProxyServer(allowedDomains: string[], opts: NetworkProxyOptions = {}): http.Server {
  const server = http.createServer((req, res) => {
    // Plain (non-TLS) HTTP request — proxy it directly.
    const hostHeader = req.headers.host ?? ""
    const [hostname, portStr] = hostHeader.split(":")
    if (!hostname || !hostAllowed(hostname, allowedDomains, opts, portStr ? Number(portStr) : 80)) {
      res.writeHead(403, { "Content-Type": "text/plain", "X-Refusal-Code": "host_not_allowed" })
      res.end(refusalText(hostname ?? "", opts))
      return
    }
    const target = http.request(
      {
        host: hostname,
        port: portStr ? Number(portStr) : 80,
        path: req.url,
        method: req.method,
        headers: req.headers,
      },
      (targetRes) => {
        res.writeHead(targetRes.statusCode ?? 502, targetRes.headers)
        targetRes.pipe(res)
      },
    )
    req.pipe(target)
    target.on("error", () => res.destroy())
  })

  server.on("connect", (req, clientSocket, head) => {
    const [hostname, portStr] = (req.url ?? "").split(":")
    const port = Number(portStr) || 443
    if (!hostname || !hostAllowed(hostname, allowedDomains, opts, port)) {
      const text = refusalText(hostname ?? "", opts)
      clientSocket.write(
        `HTTP/1.1 403 Forbidden\r\nX-Refusal-Code: host_not_allowed\r\nContent-Type: text/plain\r\nContent-Length: ${Buffer.byteLength(text)}\r\n\r\n${text}`,
      )
      clientSocket.end()
      log.warn("blocked CONNECT to non-allowlisted host", { hostname })
      return
    }
    const serverSocket = net.connect(port, hostname, () => {
      clientSocket.write("HTTP/1.1 200 Connection Established\r\n\r\n")
      if (head.length) serverSocket.write(head)
      serverSocket.pipe(clientSocket)
      clientSocket.pipe(serverSocket)
    })
    serverSocket.on("error", () => clientSocket.destroy())
    clientSocket.on("error", () => serverSocket.destroy())
  })

  return server
}

const handles = new Map<string, NetworkProxyHandle>()

export namespace NetworkProxy {
  /** Internal TCP port socat bridges to, inside the sandbox's own network namespace (Linux only). */
  export const LINUX_BRIDGE_PORT = 3129

  export async function start(
    sessionID: string,
    allowedDomains: string[],
    opts: NetworkProxyOptions = {},
  ): Promise<NetworkProxyHandle> {
    const existing = handles.get(sessionID)
    if (existing) return existing

    const server = createProxyServer(allowedDomains, opts)

    if (process.platform === "darwin") {
      await new Promise<void>((resolve, reject) => {
        server.once("error", reject)
        server.listen(0, "127.0.0.1", resolve)
      })
      const address = server.address()
      if (!address || typeof address === "string") throw new Error("Failed to bind sandbox network proxy")
      const handle: NetworkProxyHandle = {
        mode: "tcp",
        port: address.port,
        close: () => new Promise((resolve) => server.close(() => resolve())),
      }
      handles.set(sessionID, handle)
      log.info("network proxy listening", { mode: "tcp", port: address.port, sessionID })
      return handle
    }

    // Linux: listen on a unix socket, bind-mounted into the bwrap sandbox and
    // bridged to a private TCP port inside the sandbox's own netns via socat.
    const socketPath = path.join(os.tmpdir(), `gizzi-sandbox-proxy-${sessionID}.sock`)
    try {
      unlinkSync(socketPath)
    } catch {
      // didn't exist yet — fine
    }
    await new Promise<void>((resolve, reject) => {
      server.once("error", reject)
      server.listen(socketPath, resolve)
    })
    const handle: NetworkProxyHandle = {
      mode: "unix",
      socketPath,
      close: () =>
        new Promise((resolve) =>
          server.close(() => {
            try {
              unlinkSync(socketPath)
            } catch {
              // already gone
            }
            resolve()
          }),
        ),
    }
    handles.set(sessionID, handle)
    log.info("network proxy listening", { mode: "unix", socketPath, sessionID })
    return handle
  }

  export async function stop(sessionID: string): Promise<void> {
    const handle = handles.get(sessionID)
    if (!handle) return
    handles.delete(sessionID)
    await handle.close()
  }

  export function hasSocat(): boolean {
    return Boolean(Bun.which("socat"))
  }
}
