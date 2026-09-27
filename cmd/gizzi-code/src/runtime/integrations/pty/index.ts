// Mux-backed PTY integration (phase 3 of the terminal consolidation plan).
// Same namespace surface as the old bun-pty implementation, but sessions are
// real PTYs owned by the allternit-mux daemon: they survive `gizzi serve`
// restarts, keep persistent scrollback, and `connect` streams live output via
// mux event subscriptions instead of an in-process fan-out.
import { BusEvent } from "@/shared/bus/bus-event"
import { Bus } from "@/shared/bus"
import z from "zod/v4"
import { Identifier } from "@/shared/id/id"
import { Log } from "@/shared/util/log"
import { Instance } from "@/runtime/context/project/instance"
import { Shell } from "@/runtime/integrations/shell/shell"
import { Plugin } from "@/runtime/integrations/plugin"
import { connect as netConnect } from "node:net"
import { homedir } from "node:os"
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

  // ── mux NDJSON client ──────────────────────────────────────────────────────

  function muxSocketPath(): string {
    if (process.env.ALLTERNIT_MUX_SOCKET) return process.env.ALLTERNIT_MUX_SOCKET
    const stateDir = process.env.ALLTERNIT_MUX_STATE_DIR ?? join(homedir(), ".allternit", "mux")
    return join(stateDir, "mux.sock")
  }

  function muxStateDir(): string {
    return process.env.ALLTERNIT_MUX_STATE_DIR ?? join(homedir(), ".allternit", "mux")
  }

  // gizzi is a self-contained binary (gizzi-code parity): if no mux daemon is
  // running, spawn one instead of failing. Resolution order for the binary:
  // ALLTERNIT_MUX_BIN → PATH → repo target/debug (dev).
  let spawnAttempt: Promise<void> | undefined

  /** Public hook to pre-warm the mux daemon so /terminal routes served by the
   *  platform API can connect to the socket without waiting for a gizzi PTY
   *  request to lazily start it. */
  export async function warmup(): Promise<void> {
    try {
      await ensureMuxDaemon()
    } catch (err) {
      log.warn("mux warmup failed; will retry on first PTY request", { error: err })
    }
  }

  async function ensureMuxDaemon(): Promise<void> {
    const socket = muxSocketPath()
    const { existsSync, unlinkSync } = await import("node:fs")
    // NB: unix sockets are not regular files — Bun.file().exists() misses them.
    if (existsSync(socket)) {
      // Check if the existing socket is responsive. A stale socket file from a
      // dead daemon will fail to connect; remove it and respawn.
      try {
        const sock = netConnect(socket)
        await new Promise((resolve, reject) => {
          sock.once("connect", resolve)
          sock.once("error", reject)
          setTimeout(() => reject(new Error("timeout")), 1000)
        })
        sock.end()
        return
      } catch {
        try {
          unlinkSync(socket)
        } catch {
          // ignore
        }
      }
    }
    spawnAttempt ??= (async () => {
      const { spawn } = await import("node:child_process")
      const { mkdirSync } = await import("node:fs")
      const execDir = dirname(process.execPath)
      const platformArch = `${process.platform}-${process.arch}`
      const candidates = [
        // 1. Explicit override.
        process.env.ALLTERNIT_MUX_BIN,
        // 2. Vendored sibling (desktop resources/bin, dist output).
        join(execDir, "allternit-mux"),
        // 3. npm-style vendor tree (gizzi-code ripgrep layout).
        join(execDir, "vendor", "allternit-mux", platformArch, "allternit-mux"),
        // 4. PATH.
        Bun.which("allternit-mux") ?? undefined,
        // 5. Dev monorepo builds.
        join(process.cwd(), "..", "..", "target", "release", "allternit-mux"),
        join(process.cwd(), "..", "..", "target", "debug", "allternit-mux"),
      ].filter(Boolean) as string[]
      mkdirSync(muxStateDir(), { recursive: true })
      let spawned = false
      for (const bin of candidates) {
        if (!(await Bun.file(bin).exists())) continue
        try {
          const child = spawn(bin, ["serve"], {
            env: {
              ...process.env,
              ALLTERNIT_MUX_STATE_DIR: muxStateDir(),
              ALLTERNIT_MUX_SOCKET: muxSocketPath(),
            },
            detached: process.platform !== "win32",
            stdio: "ignore",
          })
          // Bun reports a failed exec on the child's error event, not sync.
          const failed = await new Promise<boolean>((resolve) => {
            child.once("error", () => resolve(true))
            setTimeout(() => resolve(false), 300)
          })
          if (failed) continue
          const { ProcessRegistry } = await import("@/runtime/process-registry")
          ProcessRegistry.track(child, { label: "allternit-mux", group: process.platform !== "win32" })
          spawned = true
          log.info("auto-spawned allternit-mux", { bin })
          break
        } catch {
          continue
        }
      }
      if (!spawned) throw new Error("allternit-mux binary not found (set ALLTERNIT_MUX_BIN)")
      // Wait for the socket to appear.
      const deadline = Date.now() + 10_000
      while (!existsSync(socket)) {
        if (Date.now() > deadline) throw new Error("allternit-mux did not start in time")
        await new Promise((r) => setTimeout(r, 100))
      }
    })().catch((e) => {
      spawnAttempt = undefined
      throw e
    })
    await spawnAttempt
  }

  class MuxConn {
    constructor(private sock: any, private pending: string[]) {}

    static connect(path: string): Promise<MuxConn> {
      return new Promise((resolve, reject) => {
        const pending: string[] = []
        const conn = new MuxConn(null as any, pending)
        const sock = netConnect(path)
        let buffer = ""
        sock.on("connect", () => resolve(conn))
        sock.on("error", reject)
        sock.on("data", (chunk: Buffer) => {
          buffer += chunk.toString("utf8")
          let idx
          while ((idx = buffer.indexOf("\n")) >= 0) {
            const line = buffer.slice(0, idx)
            buffer = buffer.slice(idx + 1)
            if (line.trim()) pending.push(line)
          }
        })
        conn.sock = sock
      })
    }

    nextLine(): Promise<string> {
      if (this.pending.length) return Promise.resolve(this.pending.shift()!)
      return new Promise((resolve, reject) => {
        const onData = () => {
          if (this.pending.length) {
            this.sock.off("data", onData)
            resolve(this.pending.shift()!)
          }
        }
        this.sock.on("data", onData)
        this.sock.once("error", reject)
        this.sock.once("close", () => reject(new Error("mux socket closed")))
      })
    }

    async request(method: string, params: Record<string, unknown> = {}): Promise<any> {
      const id = `${Date.now()}-${Math.random().toString(36).slice(2)}`
      this.sock.write(JSON.stringify({ id, method, params }) + "\n")
      for (;;) {
        const line = await this.nextLine()
        const frame = JSON.parse(line)
        if (frame.id !== id) continue
        if (frame.error) throw new Error(`${frame.error.code}: ${frame.error.message}`)
        return frame.result
      }
    }

    close(): void {
      try {
        this.sock.destroy()
      } catch {
        /* already closed */
      }
    }
  }

  async function mux<T = any>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    await ensureMuxDaemon()
    const conn = await MuxConn.connect(muxSocketPath())
    try {
      return await conn.request(method, params)
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

  // ── state: gizzi pty id -> mux session/pane mapping ───────────────────────

  interface Mapping {
    info: Info
    muxSessionId: string
    muxPaneId: string
  }

  const LABEL_PREFIX = "gizzi-pty-"

  const state = Instance.state(
    () => new Map<string, Mapping>(),
    async (sessions) => {
      for (const mapping of sessions.values()) {
        try {
          await mux("session.close", { session_id: mapping.muxSessionId })
        } catch {
          // mux may be down; sessions are daemon-owned and outlive us anyway
        }
      }
      sessions.clear()
    },
  )

  function toInfo(muxSession: any, id: string, title: string, command: string, args: string[], cwd: string): Info {
    const pane = (muxSession.panes ?? [])[0] ?? {}
    return {
      id,
      title,
      command,
      args,
      cwd,
      status: pane.process_running ? "running" : "exited",
      pid: pane.pid ?? 0,
    } as Info
  }

  async function findByLabel(id: string): Promise<any | undefined> {
    const list = await mux("session.list")
    return (list.sessions ?? []).find((s: any) => s.label === `${LABEL_PREFIX}${id}`)
  }

  // ── public API (parity with the bun-pty implementation) ────────────────────

  export async function list(): Promise<Info[]> {
    const out: Info[] = []
    for (const mapping of state().values()) {
      const session = await findByLabel(mapping.info.id).catch(() => undefined)
      if (!session) {
        out.push({ ...mapping.info, status: "exited" })
        continue
      }
      const info = toInfo(session, mapping.info.id, mapping.info.title, mapping.info.command, mapping.info.args, mapping.info.cwd)
      mapping.info = info
      out.push(info)
    }
    return out
  }

  export async function get(id: string): Promise<Info | undefined> {
    const mapping = state().get(id)
    if (!mapping) return undefined
    const session = await findByLabel(id).catch(() => undefined)
    if (!session) return { ...mapping.info, status: "exited" }
    mapping.info = toInfo(session, mapping.info.id, mapping.info.title, mapping.info.command, mapping.info.args, mapping.info.cwd)
    return mapping.info
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

    log.info("creating mux-backed session", { id, cmd: command, args, cwd })

    const { session } = await mux("session.create", {
      label: `${LABEL_PREFIX}${id}`,
      cwd,
    })
    const { pane } = await mux("pane.create", {
      session_id: session.session_id,
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
      pid: pane.pid ?? 0,
    }
    state().set(id, {
      info,
      muxSessionId: session.session_id,
      muxPaneId: pane.pane_id,
    })

    // Watch for exit so Event.Exited fires like the bun-pty version.
    void watchExit(id, pane.pane_id)

    Bus.publish(Event.Created, { info })
    return info
  }

  async function watchExit(id: string, paneId: string): Promise<void> {
    try {
      const conn = await MuxConn.connect(muxSocketPath())
      await conn.request("events.subscribe", { types: ["pane.exited"] })
      for (;;) {
        const frame = JSON.parse(await conn.nextLine())
        if (frame.type === "pane.exited" && frame.pane_id === paneId) {
          const exitCode = frame.data?.exit_code ?? -1
          const mapping = state().get(id)
          if (mapping) mapping.info = { ...mapping.info, status: "exited" }
          Bus.publish(Event.Exited, { id, exitCode })
          conn.close()
          return
        }
      }
    } catch {
      // mux unreachable — exit state will be reflected on next list/get
    }
  }

  export async function update(id: string, input: UpdateInput) {
    const mapping = state().get(id)
    if (!mapping) return
    if (input.title) {
      mapping.info = { ...mapping.info, title: input.title }
    }
    if (input.size) {
      await mux("pane.resize", {
        pane_id: mapping.muxPaneId,
        cols: input.size.cols,
        rows: input.size.rows,
      }).catch(() => undefined)
    }
    Bus.publish(Event.Updated, { info: mapping.info })
    return mapping.info
  }

  export async function remove(id: string) {
    const mapping = state().get(id)
    if (!mapping) return
    log.info("removing mux-backed session", { id })
    try {
      await mux("session.close", { session_id: mapping.muxSessionId })
    } catch {
      // already gone
    }
    state().delete(id)
    Bus.publish(Event.Deleted, { id })
  }

  export async function resize(id: string, cols: number, rows: number) {
    const mapping = state().get(id)
    if (!mapping || mapping.info.status !== "running") return
    await mux("pane.resize", { pane_id: mapping.muxPaneId, cols, rows }).catch(() => undefined)
  }

  export async function write(id: string, data: string) {
    const mapping = state().get(id)
    if (!mapping || mapping.info.status !== "running") return
    await mux("pane.send_input", { pane_id: mapping.muxPaneId, data }).catch(() => undefined)
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
    const mapping = state().get(id)
    if (!mapping) {
      ws.close()
      return
    }
    await ensureMuxDaemon()
    log.info("client connected to mux-backed session", { id })

    // Replay the persistent scrollback (survives server restarts, unlike the
    // old in-memory buffer).
    const replay = await mux("pane.read", { pane_id: mapping.muxPaneId, source: "scrollback" }).catch(() => undefined)
    const data: string = replay?.output ?? ""
    if (data) {
      try {
        for (let i = 0; i < data.length; i += 64 * 1024) {
          ws.send(data.slice(i, i + 64 * 1024))
        }
      } catch {
        ws.close()
        return
      }
    }
    try {
      ws.send(meta(data.length))
    } catch {
      ws.close()
      return
    }

    // Live output via mux event subscription on a dedicated connection.
    const conn = await MuxConn.connect(muxSocketPath()).catch(() => undefined)
    if (!conn) {
      ws.close()
      return
    }
    await conn.request("events.subscribe", { types: ["pane.output", "pane.exited"] })
    const pump = (async () => {
      try {
        for (;;) {
          const frame = JSON.parse(await conn.nextLine())
          const framePaneId = frame.pane_id ?? frame.data?.pane_id
          if (framePaneId !== mapping.muxPaneId) continue
          if (frame.type === "pane.output") {
            const chunk = frame.data?.data ?? ""
            if (chunk) {
              try {
                // Prevent cross-connection output leaks: only send if this ws
                // is still associated with the connection that created the pump.
                const currentConnection = (ws as any).data?.events?.connection ?? (ws as any).data?.connId ?? id
                if (currentConnection !== connectionId) {
                  break
                }
                ws.send(chunk)
              } catch {
                break
              }
            }
          } else if (frame.type === "pane.exited") {
            try {
              ws.close()
            } catch {
              // ignore
            }
            break
          }
        }
      } catch {
        // mux went away
      }
    })()
    void pump

    return {
      onMessage: (message: string | ArrayBuffer) => {
        void mux("pane.send_input", { pane_id: mapping.muxPaneId, data: String(message) }).catch(() => undefined)
      },
      onClose: () => {
        log.info("client disconnected from mux-backed session", { id })
        conn.close()
      },
    }
  }
}
