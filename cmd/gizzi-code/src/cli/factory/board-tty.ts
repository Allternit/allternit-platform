/**
 * `gizzi board` — the live Factory board in a plain terminal (no TUI).
 *
 * Redraws when the engine's event stream or a pane's screen stream says
 * something changed (live.ts), plus a once-a-second clock tick for the
 * elapsed column. Keys: ↑↓/jk move, 1-9 jump, n next waiting, enter attach
 * (the engine's pane view owns the screen until you detach), s steer,
 * x stop, l land, q quit.
 */
import { attachArgv, passThrough } from "@/cli/factory/engine"
import { C } from "@/cli/factory/render"
import { FactoryLive, type LiveView } from "@/cli/factory/live"
import {
  BOARD_KEYS,
  allRows,
  buildBoard,
  fit,
  rowColumns,
  summarize,
  summaryText,
  type BoardGroup,
  type BoardRow,
  type BoardState,
} from "@/cli/factory/board"
import { drainReady, landSwarm, notePending, observeSwarm, openSwarms, readSwarm, steer, stopAgent, stopWorker } from "@/cli/factory/swarm"

const STATE_COLOR: Record<BoardState, string> = {
  starting: C.dim,
  working: "\x1b[96m",
  waiting: C.yellow,
  idle: C.dim,
  done: C.green,
  failed: C.red,
  stopped: C.dim,
}

/** One frame of the board as plain lines (ANSI when `color`). Pure, for tests and `--once`. */
export function renderBoardFrame(
  groups: BoardGroup[],
  view: Pick<LiveView, "connected" | "error">,
  opts: { width: number; selected: number; color: boolean; status?: string; prompt?: string | null },
): string[] {
  const p = (code: string, s: string) => (opts.color ? `${code}${s}${C.reset}` : s)
  const w = Math.max(60, opts.width)
  const inner = w - 4
  const rows = allRows(groups)
  const sum = summarize(rows)
  const live = view.connected ? "⟳ live" : "○ offline"
  const title = ` Factory · ${summaryText(sum)} `
  const bar = "─".repeat(Math.max(1, w - 2 - [...title].length - live.length - 3))
  const out: string[] = []
  out.push(p(C.gizzi, `┌${title}${bar} ${live} ┐`))
  const line = (s: string, raw?: string) => {
    const pad = Math.max(0, inner - [...(raw ?? s)].length)
    out.push(`${p(C.gizzi, "│")} ${s}${" ".repeat(pad)} ${p(C.gizzi, "│")}`)
  }
  const head = `${fit("#", 3)} ${fit("BOT", 16)} ${fit("BRAIN", Math.min(30, Math.max(10, Math.floor(inner * 0.24))))} ${fit("STATE", 10)} ${fit("AGE", 8)} ${fit("COST", 5)} BRANCH`
  line(p(C.dim, fit(head, inner)), fit(head, inner))
  if (!view.connected && view.error) line(p(C.yellow, fit(view.error, inner)), fit(view.error, inner))
  if (!rows.length) {
    const empty = view.connected ? 'Nothing is running. Start a swarm: gizzi swarm "<task>" --workers 2 --brain claude-cli/<model>' : ""
    if (empty) line(p(C.dim, fit(empty, inner)), fit(empty, inner))
  }
  let i = 0
  for (const g of groups) {
    const t = fit(`─ ${g.title}`, inner)
    line(p(C.dim, t), t)
    for (const r of g.rows) {
      const c = rowColumns(r, i, inner)
      const sel = i === opts.selected
      const lead = sel ? "❯" : " "
      const fixed = `${c.num} ${c.bot} ${c.brain} `
      const rest = ` ${c.age} ${c.cost} `
      const branch = fit(c.branch, Math.max(4, inner - 2 - [...fixed].length - 10 - [...rest].length))
      const raw = `${lead} ${fixed}${c.state}${rest}${branch}`
      const painted = `${sel ? p(C.gizzi, lead) : lead} ${sel ? p(C.bold, fixed) : fixed}${p(STATE_COLOR[r.state], c.state)}${rest}${p(C.dim, branch)}`
      line(painted, raw)
      if (r.lastLine) {
        const t2 = fit(`    └ ${r.lastLine}`, inner)
        line(p(r.state === "waiting" ? C.yellow : C.dim, t2), t2)
      }
      i++
    }
  }
  out.push(p(C.gizzi, `└${"─".repeat(w - 2)}┘`))
  if (opts.prompt != null) out.push(` ${p(C.gizzi, "steer ›")} ${opts.prompt}█`)
  else if (opts.status) out.push(` ${opts.status.includes("\x1b") ? opts.status : fit(opts.status, w - 2).trimEnd()}`)
  else out.push(p(C.dim, ` ${BOARD_KEYS}`))
  return out
}

function nextWaiting(rows: BoardRow[], from: number): number {
  for (let k = 1; k <= rows.length; k++) {
    const j = (from + k) % rows.length
    if (rows[j]!.state === "waiting") return j
  }
  return from
}

/** `--once`: one snapshot, printed, no screen takeover (scripts, CI, tmux checks). */
export async function printBoardOnce(color: boolean): Promise<number> {
  const live = new FactoryLive({ lines: false })
  await live.start()
  const view = await new Promise<LiveView>((resolve) => {
    const t = setTimeout(() => resolve(live.current), 4000)
    live.subscribe((v) => {
      if (v.connected || (v.error && !live.url)) {
        clearTimeout(t)
        resolve(v)
      }
    })
  })
  live.stop()
  const swarms = openSwarms().map((s) => (view.connected ? observeSwarm(s, view.agents) : s))
  const groups = buildBoard({ agents: view.agents, swarms, lastLine: view.lastLine, since: view.since })
  process.stdout.write(renderBoardFrame(groups, view, { width: process.stdout.columns || 100, selected: -1, color }).join("\n") + "\n")
  return view.connected ? 0 : 3
}

export async function runBoardTty(): Promise<number> {
  if (!process.stdin.isTTY || !process.stdout.isTTY) return printBoardOnce(false)
  const out = process.stdout
  const live = new FactoryLive({ lines: true })
  let view = live.current
  let selected = 0
  let status = ""
  let prompt: string | null = null
  let confirm: { text: string; run: () => Promise<string> } | null = null
  let paused = false
  let groups: BoardGroup[] = []
  let quit: (code: number) => void = () => {}

  const draw = () => {
    if (paused) return
    const swarms = openSwarms().map((s) => (view.connected ? observeSwarm(s, view.agents) : s))
    if (view.connected) for (const s of swarms) drainReady(s, view.agents)
    groups = buildBoard({ agents: view.agents, swarms, lastLine: view.lastLine, since: view.since })
    const rows = allRows(groups)
    if (selected >= rows.length) selected = Math.max(0, rows.length - 1)
    const lines = renderBoardFrame(groups, view, {
      width: out.columns || 100,
      selected,
      color: !process.env.NO_COLOR,
      status: confirm ? `${C.yellow}${confirm.text} (y/n)${C.reset}` : status,
      prompt,
    })
    out.write("\x1b[H\x1b[2J" + lines.join("\r\n"))
  }

  const enter = () => {
    out.write("\x1b[?1049h\x1b[?25l")
    process.stdin.setRawMode(true)
    process.stdin.resume()
  }
  const leave = () => {
    out.write("\x1b[?25h\x1b[?1049l")
    process.stdin.setRawMode(false)
  }

  const act = async (label: string, fn: () => Promise<string>) => {
    status = `${label}…`
    draw()
    try {
      status = await fn()
    } catch (err) {
      const e = err as { fact?: string; action?: string | null; message?: string }
      status = `${C.red}${e.fact ?? e.message}${e.action ? ` — ${e.action}` : ""}${C.reset}`
    }
    live.refresh()
    draw()
  }

  const onKey = (buf: Buffer) => {
    const s = buf.toString("utf8")
    const rows = allRows(groups)
    const row = rows[selected]
    if (prompt != null) {
      if (s === "\r" || s === "\n") {
        const text = prompt.trim()
        prompt = null
        if (text && row) void act(`steering ${row.address}`, async () => {
          const r = await steer(row.address, text, row.swarm?.root)
          if (r.state === "queued" && row.swarm && row.worker) notePending(row.swarm, row.worker.bot)
          return `→ ${row.address}: ${r.state}${r.detail ? ` (${r.detail})` : ""}`
        })
        else draw()
      } else if (s === "\x1b" || s === "\x03") {
        prompt = null
        draw()
      } else if (s === "\x7f" || s === "\b") {
        prompt = prompt.slice(0, -1)
        draw()
      } else if (!s.startsWith("\x1b")) {
        prompt += s.replace(/[\x00-\x1f]/g, "")
        draw()
      }
      return
    }
    if (confirm) {
      const c = confirm
      confirm = null
      if (s === "y" || s === "Y") void act(c.text.replace(/\?$/, ""), c.run)
      else {
        status = "cancelled"
        draw()
      }
      return
    }
    if (s === "q" || s === "\x03" || s === "\x1b") return quit(0)
    if (s === "\x1b[A" || s === "k") selected = Math.max(0, selected - 1)
    else if (s === "\x1b[B" || s === "j") selected = Math.min(Math.max(0, rows.length - 1), selected + 1)
    else if (/^[1-9]$/.test(s)) selected = Math.min(rows.length - 1, Number(s) - 1)
    else if (s === "n") selected = nextWaiting(rows, selected)
    else if (!row) return draw()
    else if (s === "\r") {
      if (!row.attachable) {
        status = `${row.address} has no pane to attach to`
      } else {
        paused = true
        // The attached pane owns the keyboard: stop reading stdin until it detaches.
        process.stdin.off("data", onKey)
        leave()
        process.stdin.pause()
        out.write(`Attaching to ${row.address} — ctrl+b q detaches and brings the board back.\n`)
        void passThrough(row.agent ? attachArgv(row.agent) : ["agents", "attach", row.address])
          .catch((err) => {
            status = (err as Error).message
          })
          .finally(() => {
            paused = false
            process.stdin.on("data", onKey)
            enter()
            live.refresh()
            draw()
          })
        return
      }
    } else if (s === "s") {
      prompt = ""
      status = ""
    } else if (s === "x") {
      confirm = {
        text: `Stop ${row.address}?`,
        run: async () => {
          if (row.swarm && row.worker) {
            await stopWorker(readSwarm(row.swarm.id) ?? row.swarm, row.worker.bot)
            return `■ stopped ${row.address}; its worktree stays at ${row.worker.workdir} (gizzi swarm land ${row.swarm.id} --worker ${row.worker.bot} merges it)`
          }
          if (row.agent) await stopAgent(row.agent)
          return `■ stopped ${row.address}`
        },
      }
    } else if (s === "l") {
      if (!row.swarm || !row.worker) status = `${row.address} isn't a swarm worker; land its branch by hand`
      else {
        const sw = row.swarm
        const bot = row.worker.bot
        confirm = {
          text: `Land ${row.address} (merge ${row.worker.branch} into ${sw.baseBranch})?`,
          run: async () => {
            const steps = await landSwarm(readSwarm(sw.id) ?? sw, { bots: [bot] })
            const st = steps[0]
            return st ? `${st.bot}: ${st.outcome ?? st.action}${st.detail ? ` (${st.detail})` : st.outcome === "skipped" ? ` (${st.reason})` : ""}` : "nothing to land"
          },
        }
      }
    }
    draw()
  }

  enter()
  live.subscribe((v) => {
    view = v
    draw()
  })
  await live.start()
  const tick = setInterval(draw, 1000)
  process.stdin.on("data", onKey)
  const onResize = () => draw()
  out.on("resize", onResize)
  const code = await new Promise<number>((resolve) => {
    quit = resolve
  })
  clearInterval(tick)
  process.stdin.off("data", onKey)
  out.off("resize", onResize)
  live.stop()
  leave()
  process.stdin.pause()
  return code
}
