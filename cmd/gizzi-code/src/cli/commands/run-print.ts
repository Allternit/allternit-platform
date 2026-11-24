/**
 * How `gizzi run` prints tool calls (the text/TTY format; json and
 * stream-json are untouched).
 *
 * One header per tool call, printed when the call starts, then its body
 * (live command output, a diff, a short result) under it. The engine sends a
 * tool part twice (running, then completed); printing a full line for each
 * gave every tool two lines. CLI brains (claude-cli, codex-cli) run their own
 * tools and report them with their own names and argument keys (`Read` with
 * `file_path`), so names and inputs are normalized before rendering; that is
 * what printed `Read(Read)` and `Edit(false)`.
 */
import path from "path"
import chalk from "@/shared/util/chalk"
import { Highlight } from "@/shared/util/highlight"
import { TurnSummary } from "@/shared/util/turn-summary"
import { ColorDiff } from "@/vendor/color-diff-napi"
import { countHunks, hunksFromStrings, hunksFromUnified, patchFiles, unwrapShell, type Hunk } from "@/shared/util/cli-tools"

export type PrintToolPart = {
  id: string
  tool: string
  state: {
    status: "pending" | "running" | "completed" | "error"
    input?: Record<string, unknown>
    output?: string
    error?: string
    title?: string
    metadata?: Record<string, unknown>
  }
}

type Write = (line: string) => void

const coral = (s: string) => chalk.rgb(217, 119, 87)(s)
const red = (s: string) => chalk.rgb(215, 95, 95)(s)
const cyanBold = (s: string) => chalk.bold.rgb(95, 175, 215)(s)
const dim = (s: string) => chalk.dim(s)
const bold = (s: string) => chalk.bold(s)

const LIVE_MAX_LINES = 12
const RESULT_HEAD = 4
const RESULT_TAIL = 10
const DIFF_MAX_ROWS = 24
const GUTTER = dim("  │ ")

// ---------------------------------------------------------------------------
// Normalization: CLI brain tool names / argument keys → engine shape
// ---------------------------------------------------------------------------

const NAME: Record<string, string> = {
  read: "read",
  edit: "edit",
  multiedit: "edit",
  write: "write",
  bash: "bash",
  exec_command: "bash",
  shell: "bash",
  grep: "grep",
  glob: "glob",
  ls: "list",
  list: "list",
  webfetch: "webfetch",
  web_fetch: "webfetch",
  websearch: "websearch",
  web_search: "websearch",
  task: "task",
  agent: "task",
  todowrite: "todowrite",
  skill: "skill",
  patch_apply: "patch",
  apply_patch: "patch",
  notebookedit: "edit",
}

const KEY: Record<string, string> = {
  file_path: "filePath",
  notebook_path: "filePath",
  old_string: "oldString",
  new_string: "newString",
  replace_all: "replaceAll",
  subagent_type: "subagent_type",
}

export type NormalTool = { name: string; label: string; input: Record<string, unknown> }

export function normalizeTool(tool: string, input: Record<string, unknown> | undefined): NormalTool {
  const name = NAME[tool.toLowerCase()] ?? tool
  const out: Record<string, unknown> = {}
  for (const [k, v] of Object.entries(input ?? {})) out[KEY[k] ?? k] = v
  if (out.command !== undefined) out.command = unwrapShell(out.command)
  return { name, label: LABEL[name] ?? titlecase(tool), input: out }
}

const LABEL: Record<string, string> = {
  read: "Read",
  edit: "Edit",
  write: "Write",
  bash: "Bash",
  grep: "Grep",
  glob: "Glob",
  list: "List",
  webfetch: "Fetch",
  websearch: "Search",
  task: "Agent",
  todowrite: "Todos",
  skill: "Skill",
  patch: "Patch",
}

function titlecase(s: string): string {
  const words = s.replace(/^mcp__/, "").replace(/__/g, " · ").replace(/_/g, " ")
  return words.charAt(0).toUpperCase() + words.slice(1)
}

// ---------------------------------------------------------------------------
// Paths and widths
// ---------------------------------------------------------------------------

export function columns(): number {
  return Math.max(40, process.stderr.columns || process.stdout.columns || 100)
}

/** cwd-relative when inside the cwd, ~-relative under home, else as given. */
export function shortPath(input: string, cwd = process.cwd()): string {
  if (!input) return ""
  if (!path.isAbsolute(input)) return input
  const rel = path.relative(cwd, input)
  if (rel && !rel.startsWith("..") && !path.isAbsolute(rel)) return rel
  if (!rel) return "."
  const home = process.env.HOME
  if (home && input.startsWith(home + path.sep)) return "~" + input.slice(home.length)
  return input
}

/** Cut the middle only when the text does not fit. */
export function fitMiddle(text: string, max: number): string {
  if (text.length <= max) return text
  if (max <= 1) return "…"
  const keep = max - 1
  const head = Math.ceil(keep * 0.4)
  return text.slice(0, head) + "…" + text.slice(text.length - (keep - head))
}

function oneLine(s: string): string {
  return s.replace(/\s*\n\s*/g, " ⏎ ").trim()
}

// ---------------------------------------------------------------------------
// Headers
// ---------------------------------------------------------------------------

type Header = { label: string; arg?: string; argIsPath?: boolean; note?: string }

function headerFor(t: NormalTool, part: PrintToolPart): Header {
  const i = t.input
  const str = (v: unknown) => (typeof v === "string" ? v : v === undefined ? "" : String(v))
  switch (t.name) {
    case "read": {
      const range = [i.offset !== undefined ? `from ${i.offset}` : "", i.limit !== undefined ? `${i.limit} lines` : ""]
        .filter(Boolean)
        .join(", ")
      return { label: t.label, arg: shortPath(str(i.filePath)), argIsPath: true, note: range || undefined }
    }
    case "edit":
    case "write":
      return { label: t.label, arg: shortPath(str(i.filePath)), argIsPath: true }
    case "list":
      return { label: t.label, arg: i.path ? shortPath(str(i.path)) : ".", argIsPath: true }
    case "bash":
      return { label: t.label, arg: oneLine(str(i.command)), note: str(i.description) || undefined }
    case "grep":
    case "glob": {
      const where = i.path ? `in ${shortPath(str(i.path))}` : ""
      const count = part.state.metadata?.count ?? part.state.metadata?.matches
      const n = typeof count === "number" ? `${count} ${count === 1 ? "match" : "matches"}` : ""
      return { label: t.label, arg: `"${str(i.pattern)}"`, note: [where, n].filter(Boolean).join(" · ") || undefined }
    }
    case "webfetch":
      return { label: t.label, arg: str(i.url) }
    case "websearch":
      return { label: t.label, arg: `"${str(i.query)}"` }
    case "task":
      return { label: t.label, arg: titlecase(str(i.subagent_type) || "agent"), note: str(i.description) || undefined }
    case "skill":
      return { label: t.label, arg: str(i.name) }
    case "todowrite":
      return { label: t.label }
    case "patch": {
      const files = patchFiles(i.changes).map((f) => shortPath(f.path))
      return { label: t.label, arg: files.join(" "), argIsPath: files.length === 1 }
    }
  }
  // Unknown tool: the first short string argument, never a boolean flag.
  const first = Object.values(i).find((v) => typeof v === "string" && v.trim()) as string | undefined
  return { label: t.label, arg: first ? oneLine(first) : undefined }
}

function renderHeader(h: Header, opts: { failed?: boolean; counts?: string } = {}): string {
  const bullet = opts.failed ? red("✕") : coral("⏺")
  const counts = opts.counts ? `  ${opts.counts}` : ""
  const countsLen = opts.counts ? stripAnsi(counts).length : 0
  let room = columns() - 3 - h.label.length - 1 - countsLen
  let note = h.note ? `  ${h.note}` : ""
  if (h.arg && note && room - h.arg.length < note.length) note = ""
  room -= note.length
  const arg = h.arg ? fitMiddle(h.arg, Math.max(12, room)) : ""
  const argOut = arg ? " " + (h.argIsPath ? cyanBold(arg) : arg) : ""
  return `${bullet} ${bold(h.label)}${argOut}${counts}${note ? dim(note) : ""}`
}

// eslint-disable-next-line no-control-regex
const ANSI = /\u001b\[[0-9;]*m/g
function stripAnsi(s: string): string {
  return s.replace(ANSI, "")
}

// ---------------------------------------------------------------------------
// Bodies
// ---------------------------------------------------------------------------

function cleanOutput(output: string | undefined): string {
  if (!output) return ""
  return output
    .replace(/\n*<bash_metadata>[\s\S]*?<\/bash_metadata>\s*$/, "")
    .replace(/\s+$/, "")
}

function outputLines(text: string): string[] {
  return stripAnsi(text).split("\n").map((l) => l.replace(/\r$/, ""))
}

function renderDiff(hunks: Hunk[], filePath: string, write: Write) {
  const width = columns() - 4
  let rows = 0
  let hidden = 0
  for (const [n, hunk] of hunks.entries()) {
    if (n > 0 && rows < DIFF_MAX_ROWS) write(dim("    ⋮"))
    const lines =
      new ColorDiff(hunk, null, filePath, null).render("dark", width, false) ??
      hunk.lines.map((l) => `  ${l}`)
    for (const line of lines) {
      if (rows >= DIFF_MAX_ROWS) {
        hidden++
        continue
      }
      write("    " + line)
      rows++
    }
  }
  if (hidden > 0) write(dim(`    … +${hidden} lines`))
}

// ---------------------------------------------------------------------------
// Braille dot progress (media and audio generation)
// ---------------------------------------------------------------------------

const DOT_REST = "\u2802" // one dot
const DOT_FILLED = "\u28FF" // all eight dots
// Light to dense, for the soft edges of the sweeping band.
const DOT_RAMP = ["\u2802", "\u2806", "\u2816", "\u2836", "\u28B6", "\u28F6", "\u28FE"]
const BAND_HALF = 3
export const DOT_FPS = 10

/**
 * One braille line: a light band sweeping across resting dots. Pure: the same
 * (width, t, fraction) always gives the same string.
 *
 * `t` is seconds. With a real `fraction` (0 to 1) the cells it has covered are
 * solid and the band sweeps only over the rest; without one the band sweeps the
 * whole width. A missing or non-finite `t` draws the still frame (no band).
 */
export function dotBandFrame(width: number, t: number | undefined, fraction?: number): string {
  const w = Math.max(1, Math.floor(width))
  const done = fraction === undefined || !Number.isFinite(fraction) ? 0 : Math.round(Math.min(1, Math.max(0, fraction)) * w)
  const span = w - done
  let out = DOT_FILLED.repeat(done)
  if (span <= 0) return out
  const moving = t !== undefined && Number.isFinite(t)
  // The band enters left of the open stretch and leaves past its right end.
  const cycle = span + BAND_HALF * 2
  const center = moving ? ((((t as number) * 14) % cycle) + cycle) % cycle - BAND_HALF : -Infinity
  for (let i = 0; i < span; i++) {
    const d = Math.abs(i - center)
    const level = Math.max(0, Math.round((1 - d / (BAND_HALF + 1)) * (DOT_RAMP.length - 1)))
    out += d <= BAND_HALF ? DOT_RAMP[level]! : DOT_REST
  }
  return out
}

export type ProgressDetailLite = { label?: string; fraction?: number; stage?: string }

export function progressDetailOf(meta: Record<string, unknown> | undefined): ProgressDetailLite | undefined {
  const d = meta?.progress_detail
  if (d && typeof d === "object") return d as ProgressDetailLite
  if (typeof meta?.progress === "string") return { label: meta.progress }
  return undefined
}

/** The text beside the dots: the real stage label, seconds, and a percent only when the fraction is real. */
export function dotProgressText(detail: ProgressDetailLite, elapsedS: number): string {
  const bits = [detail.label || detail.stage || "Working", `${Math.max(0, Math.floor(elapsedS))}s`]
  if (typeof detail.fraction === "number" && Number.isFinite(detail.fraction)) bits.push(`${Math.round(Math.min(1, Math.max(0, detail.fraction)) * 100)}%`)
  return bits.join(" \u00B7 ")
}

/** Animate only on a real, colored terminal, and not when the user asked for less motion. */
export function shouldAnimateDots(env: Record<string, string | undefined> = process.env, isTTY: boolean = !!process.stdout.isTTY): boolean {
  if (!isTTY) return false
  if (env.NO_COLOR !== undefined && env.NO_COLOR !== "") return false
  if (env.TERM === "dumb") return false
  const reduced = env.GIZZI_REDUCED_MOTION ?? env.REDUCED_MOTION
  if (reduced && reduced !== "0" && reduced.toLowerCase() !== "false") return false
  return true
}

// ---------------------------------------------------------------------------
// The printer
// ---------------------------------------------------------------------------

type Tracked = { header: Header; printed: boolean; liveChars: number; liveLines: number; liveHidden: number; startedAt: number; lastLabel?: string }

/** Where the animated line is drawn; defaults to stdout. */
export type DotSink = { write(text: string): unknown }

export class ToolPrinter {
  private parts = new Map<string, Tracked>()
  private last = ""
  private bodyAbove = true
  private toolCount = 0
  private changes: TurnSummary.FileChange[] = []

  private dots?: { id: string; timer?: ReturnType<typeof setInterval>; drawn: boolean }
  private detail?: ProgressDetailLite

  constructor(
    private readonly write: Write,
    private readonly opts: { animate?: boolean; sink?: DotSink; now?: () => number } = {},
  ) {}

  get tools() {
    return this.toolCount
  }

  get files() {
    return TurnSummary.merge(this.changes)
  }

  private track(part: PrintToolPart, t: NormalTool): Tracked {
    let tracked = this.parts.get(part.id)
    if (!tracked) {
      tracked = { header: headerFor(t, part), printed: false, liveChars: 0, liveLines: 0, liveHidden: 0, startedAt: this.now() }
      this.parts.set(part.id, tracked)
      this.toolCount++
    }
    return tracked
  }

  /** Print the header unless it is already the line block right above. */
  private ensureHeader(part: PrintToolPart, tracked: Tracked, opts: { failed?: boolean; counts?: string } = {}) {
    if (tracked.printed && this.last === part.id && !opts.counts) return
    // Consecutive one-line tools stay together; a blank line follows a body.
    if (this.bodyAbove) this.write("")
    this.write(renderHeader(tracked.header, opts))
    this.bodyAbove = false
    tracked.printed = true
    this.last = part.id
  }

  private body(line: string) {
    this.write(line)
    this.bodyAbove = true
  }

  /** Called before other output (assistant text) so the next header gets a gap. */
  separate() {
    this.stopDots()
    this.bodyAbove = true
    this.last = ""
  }

  private now(): number {
    return (this.opts.now ?? Date.now)()
  }

  private get animate(): boolean {
    return this.opts.animate ?? shouldAnimateDots()
  }

  /** Draw the one braille line in place; static runs print a plain line when the stage changes. */
  private progress(part: PrintToolPart, tracked: Tracked) {
    const detail = progressDetailOf(part.state.metadata)
    if (!detail) return
    this.detail = detail
    if (!this.animate) {
      if (tracked.lastLabel === (detail.label ?? "")) return
      tracked.lastLabel = detail.label ?? ""
      this.body(`  ${dim("\u2837")} ${detail.label || detail.stage || "Working"}`)
      return
    }
    if (!this.dots || this.dots.id !== part.id) {
      this.stopDots()
      this.detail = detail
      this.dots = { id: part.id, drawn: false }
      this.dots.timer = setInterval(() => this.drawDots(tracked), Math.round(1000 / DOT_FPS))
      this.dots.timer.unref?.()
    }
    this.drawDots(tracked)
  }

  private drawDots(tracked: Tracked) {
    if (!this.dots || !this.detail) return
    const elapsed = (this.now() - tracked.startedAt) / 1000
    const text = dotProgressText(this.detail, elapsed)
    const room = columns() - 4 - text.length - 1
    const width = Math.max(8, Math.min(24, room))
    const line = `  ${coral(dotBandFrame(width, elapsed, this.detail.fraction))} ${dim(text)}`
    ;(this.opts.sink ?? process.stdout).write(`\r\u001b[2K${line}`)
    this.dots.drawn = true
  }

  /** Stop the animation and erase its line so the next output starts clean. */
  private stopDots() {
    if (!this.dots) return
    if (this.dots.timer) clearInterval(this.dots.timer)
    if (this.dots.drawn) (this.opts.sink ?? process.stdout).write("\r\u001b[2K")
    this.dots = undefined
    this.detail = undefined
  }

  /** A part update from the event stream. */
  update(part: PrintToolPart) {
    if (part.state.status !== "running") this.stopDots()
    const t = normalizeTool(part.tool, part.state.input)
    const status = part.state.status
    if (status === "pending") return
    const tracked = this.track(part, t)

    if (status === "running") {
      // Edits print their header with counts once the change is known.
      if (t.name !== "edit" && t.name !== "write" && t.name !== "patch") this.ensureHeader(part, tracked)
      if (t.name === "bash") this.live(part, tracked)
      else if (part.state.metadata && (t.name === "media_generate" || part.state.metadata.progress_detail)) this.progress(part, tracked)
      return
    }

    if (status === "error") {
      this.ensureHeader(part, tracked, { failed: !(tracked.printed && this.last === part.id) })
      const error = cleanOutput(part.state.error)
      // A failed command's output (test results) matters more than the bare
      // "Exit code 1"; other tools get their first error line.
      if (t.name === "bash" && error.includes("\n")) {
        const [head, ...rest] = error.split("\n")
        this.body(`  ${dim("⎿")}  ${red(head!.trim())}`)
        this.block(rest.join("\n").replace(/^\s*\n/, ""))
      } else this.body(`  ${dim("⎿")}  ${red(firstLine(error))}`)
      this.parts.delete(part.id)
      return
    }

    this.completed(part, t, tracked)
    this.parts.delete(part.id)
  }

  private live(part: PrintToolPart, tracked: Tracked) {
    const output = part.state.metadata?.output
    if (typeof output !== "string" || output.length <= tracked.liveChars) return
    // Only complete lines; a partial last line waits for the next update.
    const end = output.lastIndexOf("\n") + 1
    if (end <= tracked.liveChars) return
    const fresh = outputLines(output.slice(tracked.liveChars, end - 1))
    tracked.liveChars = end
    if (this.last !== part.id) this.ensureHeader(part, tracked)
    for (const line of fresh) {
      if (tracked.liveLines >= LIVE_MAX_LINES) {
        tracked.liveHidden++
        continue
      }
      this.body(GUTTER + Highlight.line(line))
      tracked.liveLines++
    }
  }

  private completed(part: PrintToolPart, t: NormalTool, tracked: Tracked) {
    const meta = part.state.metadata ?? {}
    const input = t.input
    switch (t.name) {
      case "edit": {
        const file = String(input.filePath ?? "")
        const unified = typeof meta.diff === "string" ? meta.diff : undefined
        let hunks: Hunk[] = unified ? hunksFromUnified(unified) : []
        if (!hunks.length && Array.isArray(input.edits)) {
          hunks = (input.edits as Array<Record<string, unknown>>).flatMap((e) =>
            hunksFromStrings(file, String(e.old_string ?? e.oldString ?? ""), String(e.new_string ?? e.newString ?? "")),
          )
        } else if (!hunks.length && (typeof input.oldString === "string" || typeof input.newString === "string")) {
          hunks = hunksFromStrings(file, String(input.oldString ?? ""), String(input.newString ?? ""))
        }
        const c = countHunks(hunks)
        this.changes.push({ path: shortPath(file), ...c })
        this.ensureHeader(part, tracked, { counts: Highlight.counts(c.added, c.removed) })
        renderDiff(hunks, file, (line) => this.body(line))
        return
      }
      case "write": {
        const file = String(input.filePath ?? "")
        const content = typeof input.content === "string" ? input.content : ""
        const added = content ? content.replace(/\n$/, "").split("\n").length : 0
        this.changes.push({ path: shortPath(file), added, removed: 0 })
        const created = meta.exists === false || /created/i.test(part.state.output ?? "")
        tracked.header.note = created ? "new file" : undefined
        this.ensureHeader(part, tracked, { counts: Highlight.counts(added, 0) })
        return
      }
      case "patch": {
        // One Edit line + diff per file, like Claude's edits.
        const files = patchFiles(input.changes)
        for (const [n, f] of files.entries()) {
          this.changes.push({ path: shortPath(f.path), added: f.added, removed: f.removed })
          const label = f.kind === "add" ? "Write" : f.kind === "delete" ? "Delete" : "Edit"
          const fileTracked = n === 0 ? tracked : { ...tracked, printed: false }
          fileTracked.header = { label, arg: shortPath(f.path), argIsPath: true, note: f.kind === "add" ? "new file" : undefined }
          this.ensureHeader({ ...part, id: `${part.id}#${n}` }, fileTracked, { counts: f.kind === "delete" ? undefined : Highlight.counts(f.added, f.removed) })
          if (f.kind === "update") renderDiff(f.hunks, f.path, (line) => this.body(line))
        }
        if (!files.length) this.ensureHeader(part, tracked)
        return
      }
      case "bash": {
        const output = cleanOutput(part.state.output)
        const exit = typeof meta.exit === "number" ? meta.exit : undefined
        if (tracked.liveLines > 0) {
          // Live lines already shown; add what was held back, then the tail.
          const all = outputLines(output)
          const remaining = all.length - tracked.liveLines
          if (remaining > 0) {
            const tail = all.slice(-Math.min(RESULT_TAIL, remaining))
            const skipped = remaining - tail.length
            if (this.last !== part.id) this.ensureHeader(part, tracked)
            if (skipped > 0) this.body(GUTTER + dim(`… +${skipped} lines`))
            for (const line of tail) this.body(GUTTER + Highlight.line(line))
          }
        } else {
          this.ensureHeader(part, tracked)
          this.block(output)
        }
        if (exit !== undefined && exit !== 0) this.body(`  ${dim("⎿")}  ${red(`exit ${exit}`)}`)
        return
      }
      case "todowrite": {
        this.ensureHeader(part, tracked)
        const todos = Array.isArray(input.todos) ? (input.todos as Array<Record<string, unknown>>) : []
        for (const item of todos) {
          const done = item.status === "completed"
          const mark = done ? chalk.rgb(135, 215, 135)("✓") : item.status === "in_progress" ? coral("◐") : dim("○")
          this.body(`  ${mark} ${done ? dim(String(item.content ?? "")) : String(item.content ?? "")}`)
        }
        return
      }
      case "read":
      case "grep":
      case "glob":
      case "list":
      case "webfetch":
      case "websearch":
      case "skill":
      case "task":
        // Their result is context for the model; the header is the record.
        // Printed when it started; print again only if it never was.
        if (!tracked.printed) {
          tracked.header = headerFor(t, part)
          this.ensureHeader(part, tracked)
        }
        return
    }
    this.ensureHeader(part, tracked)
    this.block(cleanOutput(part.state.output))
  }

  /** Command / tool output: first and last lines, gutter, highlighted. */
  private block(output: string) {
    if (!output.trim()) return
    const lines = outputLines(output)
    const show =
      lines.length <= RESULT_HEAD + RESULT_TAIL + 1
        ? lines
        : [...lines.slice(0, RESULT_HEAD), `\u0000… +${lines.length - RESULT_HEAD - RESULT_TAIL} lines`, ...lines.slice(-RESULT_TAIL)]
    const width = columns() - 4
    for (const line of show) {
      if (line.startsWith("\u0000")) {
        this.body(GUTTER + dim(line.slice(1)))
        continue
      }
      this.body(GUTTER + Highlight.line(line.length > width ? line.slice(0, width - 1) + "…" : line))
    }
  }
}

function firstLine(s: string | undefined): string {
  return (s ?? "failed").split("\n").map((l) => l.trim()).find(Boolean) ?? "failed"
}
