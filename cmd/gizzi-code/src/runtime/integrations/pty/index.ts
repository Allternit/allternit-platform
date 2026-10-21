// PTY integration on the Allternit Factory pane engine. Same namespace
// surface as the old bun-pty and allternit-mux implementations, but every
// session is a pane in the pane engine (`allternit-factory pane`, the
// Factory's agent session): a Gizzi PTY, a Desktop terminal tile and a Factory
// agent pane are the same thing. Sessions survive `gizzi serve` restarts and
// UI reconnects, replay their kept scrollback (up to 2 MiB) on connect, and
// stream live output from the pane's raw output tap.
//
// Wire: the pane engine socket, one JSON request per connection, hidden
// `factory.terminal.*` methods (factory/pane/src/factory_terminal.rs).
import { BusEvent } from "@/shared/bus/bus-event"
import { Bus } from "@/shared/bus"
import z from "zod/v4"
import { Identifier } from "@/shared/id/id"
import { Log } from "@/shared/util/log"
import { Instance } from "@/runtime/context/project/instance"
import { Shell } from "@/runtime/integrations/shell/shell"
import { Plugin } from "@/runtime/integrations/plugin"
import { connect as netConnect, type Socket as NetSocket } from "node:net"
import { existsSync } from "node:fs"
import { dirname, join } from "node:path"

export namespace Pty {
  const log = Log.create({ service: "pty" })

  const encoder = new TextEncoder()

  type Socket = {
    readyState: number
    data?: unknown
    send: (data: string | Uint8Array | ArrayBuffer) => void
    close: (code?: number, reason?: string) => void
  }

  // ── pane engine client ─────────────────────────────────────────────────────

  /** The `allternit-factory` binary: ALLTERNIT_FACTORY_BIN → next to gizzi
   *  (Desktop resources/bin, dist) → npm-style vendor tree → PATH → dev
   *  monorepo builds. */
  export function factoryBinary(): string | undefined {
    const execDir = dirname(process.execPath)
    const platformArch = `${process.platform}-${process.arch}`
    const candidates = [
      process.env.ALLTERNIT_FACTORY_BIN,
      join(execDir, "allternit-factory"),
      join(execDir, "vendor", "allternit-factory", platformArch, "allternit-factory"),
      Bun.which("allternit-factory") ?? undefined,
      join(process.cwd(), "..", "..", "target", "release", "allternit-factory"),
      join(process.cwd(), "..", "..", "target", "debug", "allternit-factory"),
    ].filter(Boolean) as string[]
    return candidates.find((bin) => existsSync(bin))
  }

  let socketPath: string | undefined = process.env.ALLTERNIT_FACTORY_PANE_SOCKET || undefined
  let ensuring: Promise<string> | undefined

  /** `allternit-factory pane tty ensure`: starts the pane engine when it is
   *  down (it daemonizes and outlives gizzi; it is the Factory's) and reports
   *  its socket. */
  async function ensureEngine(): Promise<string> {
    ensuring ??= (async () => {
      const bin = factoryBinary()
      if (!bin) throw new Error("allternit-factory binary not found (set ALLTERNIT_FACTORY_BIN)")
      const proc = Bun.spawn([bin, "pane", "tty", "ensure"], { stdin: "ignore", stdout: "pipe", stderr: "pipe" })
      const [out, err, code] = await Promise.all([
        new Response(proc.stdout).text(),
        new Response(proc.stderr).text(),
        proc.exited,
      ])
      if (code !== 0) throw new Error(`the Factory pane engine could not be started: ${err.trim() || `exit ${code}`}`)
      const socket = JSON.parse(out).socket
      if (typeof socket !== "string") throw new Error("pane tty ensure reported no socket")
      socketPath = socket
      return socket
    })().finally(() => {
      ensuring = undefined
    })
    return ensuring
  }

  /** Public hook to pre-start the pane engine so allternit-api's /terminal
   *  routes and the first PTY request don't wait for it. */
  export async function warmup(): Promise<void> {
    try {
      await ensureEngine()
    } catch (err) {
      log.warn("pane engine warmup failed; will retry on first PTY request", { error: err })
    }
  }

  function dial(path: string): Promise<NetSocket> {
    return new Promise((resolve, reject) => {
      const sock = netConnect(path)
      sock.once("connect", () => {
        sock.off("error", reject)
        resolve(sock)
      })
      sock.once("error", reject)
    })
  }

  /** Line reader over one connection. */
  class Conn {
    private pending: string[] = []
    private buffer = ""
    private waiter: ((line: string | undefined) => void) | undefined
    private ended = false

    constructor(readonly sock: NetSocket) {
      sock.on("data", (chunk: Buffer) => {
        this.buffer += chunk.toString("utf8")
        let idx
        while ((idx = this.buffer.indexOf("\n")) >= 0) {
          const line = this.buffer.slice(0, idx)
          this.buffer = this.buffer.slice(idx + 1)
          if (line.trim()) this.push(line)
        }
      })
      const end = () => {
        this.ended = true
        this.waiter?.(undefined)
        this.waiter = undefined
      }
      sock.on("close", end)
      sock.on("error", end)
    }

    private push(line: string) {
      if (this.waiter) {
        const w = this.waiter
        this.waiter = undefined
        w(line)
      } else this.pending.push(line)
    }

    /** Next line, or undefined once the connection ended. */
    next(): Promise<string | undefined> {
      if (this.pending.length) return Promise.resolve(this.pending.shift())
      if (this.ended) return Promise.resolve(undefined)
      return new Promise((resolve) => (this.waiter = resolve))
    }

    close(): void {
      try {
        this.sock.destroy()
      } catch {
        /* already closed */
      }
    }
  }

  /** Opens a connection and sends one request; returns the connection
   *  positioned at the response line. Re-resolves the socket once when the
   *  engine isn't reachable (restarted, or never started). */
  async function open(method: string, params: Record<string, unknown>): Promise<Conn> {
    let sock: NetSocket | undefined
    if (socketPath) sock = await dial(socketPath).catch(() => undefined)
    if (!sock) sock = await dial(await ensureEngine())
    const conn = new Conn(sock)
    const id = `${Date.now()}-${Math.random().toString(36).slice(2)}`
    sock.write(JSON.stringify({ id, method, params }) + "\n")
    return conn
  }

  class EngineError extends Error {
    constructor(
      readonly code: string,
      message: string,
    ) {
      super(`${code}: ${message}`)
    }
  }

  async function readResult(conn: Conn): Promise<any> {
    const line = await conn.next()
    if (line === undefined) throw new Error("the pane engine closed the connection")
    const frame = JSON.parse(line)
    if (frame.error) throw new EngineError(frame.error.code, frame.error.message)
    return frame.result
  }

  async function engine<T = any>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    const conn = await open(method, params)
    try {
      return await readResult(conn)
    } finally {
      conn.close()
    }
  }

  // ── types (unchanged surface) ──────────────────────────────────────────────

  export const Info = z.object({
    id: Identifier.schema("pty"),
    title: z.string(),
    command: z.string(),
    args: z.array(z.string()),
    cwd: z.string(),
    status: z.enum(["running", "exited"]),
    pid: z.number(),
  })

  export type Info = z.infer<typeof Info>

  export const CreateInput = z.object({
    command: z.string().optional(),
    args: z.array(z.string()).optional(),
    cwd: z.string().optional(),
    title: z.string().optional(),
    env: z.record(z.string(), z.string()).optional(),
  })

  export type CreateInput = z.infer<typeof CreateInput>

  export const UpdateInput = z.object({
    title: z.string().optional(),
    size: z
      .object({
        rows: z.number(),
        cols: z.number(),
      })
      .optional(),
  })

  export type UpdateInput = z.infer<typeof UpdateInput>

  export const Event = {
    Created: BusEvent.define("pty.created", z.object({ info: Info })),
    Updated: BusEvent.define("pty.updated", z.object({ info: Info })),
    Exited: BusEvent.define("pty.exited", z.object({ id: Identifier.schema("pty"), exitCode: z.number() })),
    Deleted: BusEvent.define("pty.deleted", z.object({ id: Identifier.schema("pty") })),
  }

  // ── state: gizzi pty ids created by this instance ────────────────────────
  // The pty id is the pane engine's terminal id, so nothing else is mapped.

  const state = Instance.state(
    () => new Map<string, Info>(),
    async (sessions) => {
      for (const id of sessions.keys()) {
        try {
          await engine("factory.terminal.close", { terminal_id: id })
        } catch {
          // already gone, or the engine is down
        }
      }
      sessions.clear()
    },
  )

  /** Live facts for one terminal; undefined when the engine no longer has it. */
  async function lookup(id: string): Promise<any | undefined> {
    try {
      const { terminal } = await engine("factory.terminal.get", { terminal_id: id })
      return terminal
    } catch (err) {
      if (err instanceof EngineError && err.code === "terminal_not_found") return undefined
      throw err
    }
  }

  function refresh(info: Info, terminal: any | undefined): Info {
    return {
      ...info,
      status: terminal?.running ? "running" : "exited",
      pid: terminal?.pid ?? info.pid,
    }
  }

  // ── public API (parity with the bun-pty implementation) ────────────────────

  export async function list(): Promise<Info[]> {
    const out: Info[] = []
    for (const [id, info] of state()) {
      const next = refresh(info, await lookup(id).catch(() => undefined))
      state().set(id, next)
      out.push(next)
    }
    return out
  }

  export async function get(id: string): Promise<Info | undefined> {
    const info = state().get(id)
    if (!info) return undefined
    const next = refresh(info, await lookup(id).catch(() => undefined))
    state().set(id, next)
    return next
  }

  export async function create(input: CreateInput) {
    const id = Identifier.create("pty", false)
    const command = input.command || Shell.preferred()
    const args = input.args || []
    if (command.endsWith("sh")) {
      args.push("-l")
    }

    const cwd = input.cwd || Instance.directory
    const shellEnv = await Plugin.trigger(
      "shell.env",
      { cwd },
      { env: {} },
    )
    const env = {
      ...input.env,
      ...shellEnv.env,
      TERM: "xterm-256color",
      GIZZI_TERMINAL: "1",
    } as Record<string, string>

    log.info("creating pane-engine terminal", { id, cmd: command, args, cwd })

    const { terminal } = await engine("factory.terminal.create", {
      terminal_id: id,
      label: `gizzi-${id.slice(-8)}`,
      cwd,
      cols: 80,
      rows: 24,
      command: [command, ...args],
      env,
    })

    const info: Info = {
      id,
      title: input.title || `Terminal ${id.slice(-4)}`,
      command,
      args,
      cwd,
      status: "running",
      pid: terminal?.pid ?? 0,
    }
    state().set(id, info)

    // Watch for exit so Event.Exited fires like the bun-pty version.
    void watchExit(id)

    Bus.publish(Event.Created, { info })
    return info
  }

  async function watchExit(id: string): Promise<void> {
    let conn: Conn | undefined
    try {
      conn = await open("factory.terminal.output", { terminal_id: id, replay: false, follow: true })
      await readResult(conn)
      for (;;) {
        const line = await conn.next()
        if (line === undefined) return // engine went away: list/get report it
        const frame = JSON.parse(line)
        if (frame.type === "exit") {
          const info = state().get(id)
          if (info) state().set(id, { ...info, status: "exited" })
          Bus.publish(Event.Exited, { id, exitCode: frame.exit_code ?? -1 })
          return
        }
      }
    } catch {
      // engine unreachable — exit state will be reflected on next list/get
    } finally {
      conn?.close()
    }
  }

  export async function update(id: string, input: UpdateInput) {
    const info = state().get(id)
    if (!info) return
    let next = info
    if (input.title) {
      next = { ...info, title: input.title }
      state().set(id, next)
    }
    if (input.size) {
      await engine("factory.terminal.resize", {
        terminal_id: id,
        cols: input.size.cols,
        rows: input.size.rows,
      }).catch(() => undefined)
    }
    Bus.publish(Event.Updated, { info: next })
    return next
  }

  export async function remove(id: string) {
    if (!state().has(id)) return
    log.info("removing pane-engine terminal", { id })
    try {
      await engine("factory.terminal.close", { terminal_id: id })
    } catch {
      // already gone
    }
    state().delete(id)
    Bus.publish(Event.Deleted, { id })
  }

  export async function resize(id: string, cols: number, rows: number) {
    const info = state().get(id)
    if (!info || info.status !== "running") return
    await engine("factory.terminal.resize", { terminal_id: id, cols, rows }).catch(() => undefined)
  }

  export async function write(id: string, data: string) {
    const info = state().get(id)
    if (!info || info.status !== "running") return
    await engine("factory.terminal.write", { terminal_id: id, data }).catch(() => undefined)
  }

  // WebSocket control frame: 0x00 + UTF-8 JSON (kept for protocol parity).
  const meta = (cursor: number) => {
    const json = JSON.stringify({ cursor })
    const bytes = encoder.encode(json)
    const out = new Uint8Array(bytes.length + 1)
    out[0] = 0
    out.set(bytes, 1)
    return out
  }

  export async function connect(id: string, ws: Socket, _cursor?: number) {
    // Capture the connection identifier BEFORE any async work. If the caller
    // reuses the same ws object for another connection, the pump must still
    // know which connection it belongs to.
    const connectionId = (ws as any).data?.events?.connection ?? (ws as any).data?.connId ?? id
    if (!state().has(id)) {
      ws.close()
      return
    }
    log.info("client connected to pane-engine terminal", { id })

    // One stream: the kept scrollback, a `live` marker, then live output.
    const conn = await open("factory.terminal.output", { terminal_id: id, replay: true, follow: true }).catch(
      () => undefined,
    )
    if (!conn) {
      ws.close()
      return
    }
    try {
      await readResult(conn)
    } catch {
      conn.close()
      ws.close()
      return
    }

    const stillOurs = () =>
      ((ws as any).data?.events?.connection ?? (ws as any).data?.connId ?? id) === connectionId
    const pump = (async () => {
      let replayed = 0
      try {
        for (;;) {
          const line = await conn.next()
          if (line === undefined) break // engine went away
          const frame = JSON.parse(line)
          // Prevent cross-connection output leaks: only send if this ws is
          // still associated with the connection that created the pump.
          if (!stillOurs()) break
          if (frame.type === "output") {
            const chunk: string = frame.data ?? ""
            if (!chunk) continue
            replayed += chunk.length
            try {
              ws.send(chunk)
            } catch {
              break
            }
          } else if (frame.type === "live") {
            try {
              ws.send(meta(replayed))
            } catch {
              break
            }
          } else if (frame.type === "exit") {
            try {
              ws.close()
            } catch {
              // ignore
            }
            break
          }
        }
      } catch {
        // engine went away
      } finally {
        conn.close()
      }
    })()
    void pump

    return {
      onMessage: (message: string | ArrayBuffer) => {
        const data = typeof message === "string" ? message : new TextDecoder().decode(message)
        void engine("factory.terminal.write", { terminal_id: id, data }).catch(() => undefined)
      },
      onClose: () => {
        log.info("client disconnected from pane-engine terminal", { id })
        conn.close()
      },
    }
  }
}
