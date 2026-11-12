/**
 * run_code — run Python on the files the person attached, in the OS sandbox.
 *
 * Each call gets `<data>/runs/<session>/<call>/` with `in/` (the attachments,
 * staged from data URLs, uploads or http URLs) and `out/` (what the script
 * saves). The script runs with the uv-managed venv's python (runtime/python/env.ts)
 * inside the same sandbox as Bash (bwrap / sandbox-exec), network OFF, write
 * access to the run dir plus the sandbox's temp dirs, a clean environment
 * (no API keys), MPLBACKEND=Agg, and `ulimit -v` on Linux. stdout/stderr are
 * capped at 64 KB each; files in `out/` at 20 MB in total.
 *
 * The result lists `files[] {name, mediaType, path}`. allternit-api, which
 * shares this host, ingests those paths into uploads on the tool-result event
 * and sends clients `files[] {name, mediaType, uploadId}` (gizzi_chat_stream.rs
 * `add_run_code_fields`).
 */
import z from "zod/v4"
import path from "path"
import fs from "fs/promises"
import { spawn, type ChildProcess } from "child_process"
import { fileURLToPath } from "url"
import { Tool } from "@/runtime/tools/builtins/tool"
import DESCRIPTION from "@/runtime/tools/builtins/run_code.txt"
import { Global } from "@/runtime/context/global"
import { Sandbox } from "@/runtime/integrations/shell/sandbox"
import { Shell } from "@/runtime/integrations/shell/shell"
import { ProcessRegistry } from "@/runtime/process-registry"
import { PythonEnv } from "@/runtime/python/env"
import { platformApiBase, platformFetch } from "@/runtime/bots/platform-api"
import type { MessageV2 } from "@/runtime/session/message-v2"
import { Log } from "@/shared/util/log"

const log = Log.create({ service: "run-code" })

export const RUN_CODE_DEFAULT_TIMEOUT_S = 60
export const RUN_CODE_MAX_TIMEOUT_S = 120
const MAX_STREAM_BYTES = 64 * 1024
const MAX_OUT_BYTES = 20 * 1024 * 1024
const MAX_OUT_FILES = 20
const MAX_INPUT_FILES = 20
const MAX_INPUT_BYTES = 100 * 1024 * 1024
const MAX_CODE_CHARS = 200_000
/** Linux address-space cap (KB): numpy/BLAS reserve a lot of virtual memory. */
const LINUX_VMEM_KB = 8 * 1024 * 1024

export interface RunCodeFile {
  name: string
  mediaType: string
  size: number
  /** Absolute path on this host (allternit-api ingests it into uploads). */
  path: string
}

export interface RunCodeMetadata {
  stdout: string
  stderr: string
  exit_code: number | null
  files: RunCodeFile[]
  inputs: string[]
  workdir: string
  setup: string[]
  timed_out: boolean
  truncated: false
}

const MEDIA_TYPES: Record<string, string> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
  svg: "image/svg+xml",
  pdf: "application/pdf",
  csv: "text/csv",
  tsv: "text/tab-separated-values",
  json: "application/json",
  txt: "text/plain",
  log: "text/plain",
  md: "text/markdown",
  html: "text/html",
  htm: "text/html",
  xlsx: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
  xls: "application/vnd.ms-excel",
  parquet: "application/vnd.apache.parquet",
  zip: "application/zip",
}

const EXT_FOR_MIME: Record<string, string> = {
  "text/csv": "csv",
  "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet": "xlsx",
  "application/vnd.ms-excel": "xls",
  "application/json": "json",
  "text/plain": "txt",
  "image/png": "png",
  "image/jpeg": "jpg",
  "application/pdf": "pdf",
}

export function mediaTypeFor(name: string): string {
  const ext = name.includes(".") ? name.slice(name.lastIndexOf(".") + 1).toLowerCase() : ""
  return MEDIA_TYPES[ext] ?? "application/octet-stream"
}

/** A safe single-segment filename. */
function safeName(raw: string | undefined, fallback: string): string {
  const base = (raw ?? "").split(/[\\/]/).pop() ?? ""
  const cleaned = base.replace(/[^\w.\- ()]+/g, "_").replace(/^\.+/, "").trim().slice(0, 120)
  return cleaned || fallback
}

/** Run folders older than this are removed (their files were ingested into uploads when the call settled). */
const RUN_RETENTION_MS = 7 * 24 * 60 * 60 * 1000
let lastPrune = 0

/** Best-effort: drop `runs/<session>/<call>` folders past retention, at most hourly. */
async function pruneOldRuns(runsDir: string): Promise<void> {
  if (Date.now() - lastPrune < 60 * 60 * 1000) return
  lastPrune = Date.now()
  const cutoff = Date.now() - RUN_RETENTION_MS
  for (const session of await fs.readdir(runsDir).catch(() => [] as string[])) {
    const sessionDir = path.join(runsDir, session)
    const calls = await fs.readdir(sessionDir).catch(() => [] as string[])
    for (const call of calls) {
      const dir = path.join(sessionDir, call)
      const stat = await fs.stat(dir).catch(() => undefined)
      if (stat && stat.mtimeMs < cutoff) await fs.rm(dir, { recursive: true, force: true }).catch(() => {})
    }
    if ((await fs.readdir(sessionDir).catch(() => [""])).length === 0) await fs.rmdir(sessionDir).catch(() => {})
  }
}

const segment = (s: string) => s.replace(/[^A-Za-z0-9_-]+/g, "_").slice(0, 80) || "x"

interface Attachment {
  name: string
  part: MessageV2.FilePart
}

/**
 * The attachments to stage. Without `select`: the file parts of the latest
 * user message that has any. With `select`: those names from any user
 * message in the session (latest wins on a name clash).
 */
function pickAttachments(messages: MessageV2.WithParts[], select?: string[]): Attachment[] {
  const users = messages.filter((m) => m.info.role === "user")
  const named = (msg: MessageV2.WithParts) =>
    msg.parts
      .filter((p): p is MessageV2.FilePart => p.type === "file")
      .map((part, i) => ({
        name: safeName(part.filename ?? nameFromUrl(part.url), `file-${i + 1}.${EXT_FOR_MIME[part.mime] ?? "bin"}`),
        part,
      }))
  if (!select?.length) {
    for (let i = users.length - 1; i >= 0; i--) {
      const files = named(users[i]!)
      if (files.length) return dedupe(files)
    }
    return []
  }
  const all = dedupe(users.flatMap(named))
  const wanted = new Set(select.map((s) => safeName(s, s).toLowerCase()))
  const picked = all.filter((a) => wanted.has(a.name.toLowerCase()))
  const missing = select.filter((s) => !picked.some((a) => a.name.toLowerCase() === safeName(s, s).toLowerCase()))
  if (missing.length) {
    const have = all.map((a) => a.name)
    throw new Error(
      `No attachment named ${missing.map((m) => `"${m}"`).join(", ")}. ` +
        (have.length ? `Attached files: ${have.join(", ")}.` : "No files are attached in this chat."),
    )
  }
  return picked
}

function dedupe(list: Attachment[]): Attachment[] {
  const byName = new Map<string, Attachment>()
  for (const a of list) byName.set(a.name.toLowerCase(), a)
  return [...byName.values()]
}

function nameFromUrl(url: string): string | undefined {
  if (url.startsWith("data:")) return undefined
  try {
    return decodeURIComponent(new URL(url, "http://x").pathname.split("/").pop() ?? "") || undefined
  } catch {
    return undefined
  }
}

const UPLOAD_PREFIX = "/api/v1/uploads/"

/** The bytes behind a file part's url (data:, upload path, http(s), file:). */
async function readAttachment(url: string, signal: AbortSignal): Promise<Buffer> {
  if (url.startsWith("data:")) {
    const comma = url.indexOf(",")
    if (comma < 0) throw new Error("malformed data URL")
    const header = url.slice(5, comma)
    const body = url.slice(comma + 1)
    return header.endsWith(";base64") ? Buffer.from(body, "base64") : Buffer.from(decodeURIComponent(body))
  }
  if (url.startsWith("file:")) return fs.readFile(fileURLToPath(url))
  const fetchSignal = AbortSignal.any([signal, AbortSignal.timeout(60_000)])
  // Uploads live on the allternit-api this gizzi talks to; send its credential.
  const base = new URL(platformApiBase())
  const parsed = new URL(url, base)
  let res: Response
  if (parsed.origin === base.origin && parsed.pathname.startsWith(UPLOAD_PREFIX)) {
    const rel = parsed.pathname + parsed.search
    res = await platformFetch("GET", rel, undefined, { signal: fetchSignal, accept: "*/*" }).catch(() =>
      fetch(`${platformApiBase()}${rel}`, { signal: fetchSignal }),
    )
  } else if (parsed.protocol === "http:" || parsed.protocol === "https:") {
    res = await fetch(parsed, { signal: fetchSignal, redirect: "follow" })
  } else {
    throw new Error(`unsupported URL scheme ${parsed.protocol}`)
  }
  if (!res.ok) throw new Error(`HTTP ${res.status}`)
  return Buffer.from(await res.arrayBuffer())
}

/** Regular files under `out/` (recursive, symlinks skipped), within the caps. */
async function collectOut(outDir: string): Promise<{ files: RunCodeFile[]; skipped: string[] }> {
  const files: RunCodeFile[] = []
  const skipped: string[] = []
  let total = 0
  const walk = async (dir: string, depth: number) => {
    const entries = await fs.readdir(dir, { withFileTypes: true }).catch(() => [])
    entries.sort((a, b) => a.name.localeCompare(b.name))
    for (const entry of entries) {
      const full = path.join(dir, entry.name)
      const rel = path.relative(outDir, full).split(path.sep).join("/")
      if (entry.isDirectory() && depth < 4) {
        await walk(full, depth + 1)
        continue
      }
      if (!entry.isFile()) continue
      const stat = await fs.lstat(full)
      if (files.length >= MAX_OUT_FILES || total + stat.size > MAX_OUT_BYTES) {
        skipped.push(rel)
        continue
      }
      total += stat.size
      files.push({ name: rel, mediaType: mediaTypeFor(rel), size: stat.size, path: full })
    }
  }
  await walk(outDir, 0)
  return { files, skipped }
}

/** Collects a stream up to `cap` bytes and remembers whether more came. */
function capped(cap: number) {
  const chunks: Buffer[] = []
  let size = 0
  let dropped = 0
  return {
    push(chunk: Buffer) {
      if (size >= cap) {
        dropped += chunk.length
        return
      }
      const take = chunk.subarray(0, cap - size)
      chunks.push(take)
      size += take.length
      dropped += chunk.length - take.length
    },
    text() {
      const body = Buffer.concat(chunks).toString("utf8")
      return dropped ? `${body}\n…(${dropped} more bytes not shown; output is capped at ${cap / 1024} KB)` : body
    },
  }
}

const parameters = z.object({
  language: z.literal("python").describe("Only python is supported"),
  code: z.string().min(1).max(MAX_CODE_CHARS).describe("The Python script. Read inputs from in/, save files to out/."),
  files: z
    .array(z.string())
    .max(MAX_INPUT_FILES)
    .optional()
    .describe("Attachment filenames to stage into in/. Omit to stage the latest message's attachments."),
  timeout_s: z
    .number()
    .positive()
    .max(RUN_CODE_MAX_TIMEOUT_S)
    .optional()
    .describe(`Time limit in seconds (default ${RUN_CODE_DEFAULT_TIMEOUT_S}, max ${RUN_CODE_MAX_TIMEOUT_S})`),
})

export const RunCodeTool = Tool.define<typeof parameters, RunCodeMetadata>("run_code", {
  description: DESCRIPTION,
  parameters,
  async execute(params, ctx) {
    if (process.platform === "win32") {
      throw new Error("run_code needs the OS sandbox (macOS or Linux); it is not available on Windows yet.")
    }
    const driver = Sandbox.detect()
    if (driver === "none") {
      throw new Error(
        "run_code only runs inside the OS sandbox and none is available. Install bubblewrap (e.g. `apt install bubblewrap`).",
      )
    }

    await ctx.ask({
      permission: "run_code",
      patterns: ["python"],
      always: ["*"],
      metadata: { language: params.language, files: params.files },
    })

    const callKey = segment(ctx.callID || `run-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`)
    const runsDir = path.join(Global.Path.data, "runs")
    void pruneOldRuns(runsDir)
    const workdir = path.join(runsDir, segment(ctx.sessionID), callKey)
    const inDir = path.join(workdir, "in")
    const outDir = path.join(workdir, "out")
    const tmpDir = path.join(workdir, "tmp")
    await fs.rm(workdir, { recursive: true, force: true })
    await Promise.all([inDir, outDir, tmpDir, path.join(workdir, ".mpl")].map((d) => fs.mkdir(d, { recursive: true })))

    // Stage attachments into in/.
    const attachments = pickAttachments(ctx.messages ?? [], params.files)
    const inputs: string[] = []
    let inputBytes = 0
    for (const a of attachments) {
      let bytes: Buffer
      try {
        bytes = await readAttachment(a.part.url, ctx.abort)
      } catch (error) {
        throw new Error(`Could not read the attachment "${a.name}": ${error instanceof Error ? error.message : error}`)
      }
      inputBytes += bytes.length
      if (inputBytes > MAX_INPUT_BYTES) throw new Error(`Attachments exceed ${MAX_INPUT_BYTES / 1024 / 1024} MB in total`)
      await fs.writeFile(path.join(inDir, a.name), bytes)
      inputs.push(a.name)
    }

    // Python env (one-time setup is the only network use).
    ctx.metadata({ title: "Preparing Python" })
    const env = await PythonEnv.ensure({
      signal: ctx.abort,
      onStatus: (line) => ctx.metadata({ title: line }),
    })
    await fs.writeFile(path.join(workdir, "main.py"), params.code)

    const childEnv: NodeJS.ProcessEnv = {
      PATH: [path.dirname(env.python), "/usr/bin", "/bin"].join(path.delimiter),
      HOME: workdir,
      TMPDIR: tmpDir,
      LANG: "C.UTF-8",
      LC_ALL: "C.UTF-8",
      MPLBACKEND: "Agg",
      MPLCONFIGDIR: path.join(workdir, ".mpl"),
      PYTHONDONTWRITEBYTECODE: "1",
      PYTHONUNBUFFERED: "1",
      INPUT_DIR: inDir,
      OUTPUT_DIR: outDir,
      RUN_CODE_PYTHON: env.python,
    }
    const command =
      (process.platform === "linux" ? `ulimit -v ${LINUX_VMEM_KB} 2>/dev/null; ` : "") +
      `exec "$RUN_CODE_PYTHON" -I main.py`
    const sandboxKey = `run_code-${segment(ctx.sessionID)}-${callKey}`
    const wrapped = await Sandbox.wrap({
      command,
      shell: "/bin/sh",
      cwd: workdir,
      sessionID: sandboxKey,
      policy: { allowWritePaths: [workdir], allowNetwork: false, allowReadPaths: [PythonEnv.root()] },
    })
    if (!wrapped) throw new Error("The sandbox became unavailable; run_code did not run.")

    ctx.metadata({ title: "Running Python" })
    const timeoutS = params.timeout_s ?? RUN_CODE_DEFAULT_TIMEOUT_S
    const stdout = capped(MAX_STREAM_BYTES)
    const stderr = capped(MAX_STREAM_BYTES)
    let timedOut = false
    let exited = false
    let exitCode: number | null = null
    try {
      // The child PATH is minimal; find bwrap / sandbox-exec on gizzi's own PATH.
      const bin = Bun.which(wrapped.bin) ?? wrapped.bin
      const proc: ChildProcess = spawn(bin, wrapped.args, {
        cwd: workdir,
        env: childEnv,
        stdio: ["ignore", "pipe", "pipe"],
        detached: true,
      })
      ProcessRegistry.track(proc, { label: "run-code", group: true })
      proc.stdout?.on("data", (c: Buffer) => stdout.push(c))
      proc.stderr?.on("data", (c: Buffer) => stderr.push(c))
      const kill = () => Shell.killTree(proc, { exited: () => exited })
      const timer = setTimeout(() => {
        timedOut = true
        void kill()
      }, timeoutS * 1000)
      const onAbort = () => void kill()
      ctx.abort.addEventListener("abort", onAbort, { once: true })
      exitCode = await new Promise<number | null>((resolve, reject) => {
        proc.once("error", reject)
        proc.once("close", (code) => {
          exited = true
          resolve(code)
        })
      }).finally(() => {
        clearTimeout(timer)
        ctx.abort.removeEventListener("abort", onAbort)
      })
    } finally {
      await Sandbox.cleanupProfile(sandboxKey)
    }

    const { files, skipped } = await collectOut(outDir)
    let errText = stderr.text()
    if (timedOut) errText += `${errText ? "\n" : ""}Stopped: the script ran past its ${timeoutS}s limit.`
    if (skipped.length) {
      errText += `${errText ? "\n" : ""}Not returned (over ${MAX_OUT_FILES} files or ${MAX_OUT_BYTES / 1024 / 1024} MB): ${skipped.join(", ")}`
    }
    const metadata: RunCodeMetadata = {
      stdout: stdout.text(),
      stderr: errText,
      exit_code: timedOut ? null : exitCode,
      files,
      inputs,
      workdir,
      setup: env.setup,
      timed_out: timedOut,
      truncated: false,
    }
    log.info("run_code finished", { exit: metadata.exit_code, files: files.length, timedOut })

    const output = JSON.stringify(
      {
        exit_code: metadata.exit_code,
        stdout: metadata.stdout,
        stderr: metadata.stderr,
        files: files.map((f) => ({ name: f.name, mediaType: f.mediaType, size: f.size })),
        inputs,
        ...(env.setup.length ? { setup: env.setup } : {}),
      },
      null,
      2,
    )
    const status = timedOut ? "timed out" : exitCode === 0 ? "ok" : `exit ${exitCode}`
    return {
      title: `Python: ${status}${files.length ? `, ${files.length} file${files.length > 1 ? "s" : ""}` : ""}`,
      output,
      metadata,
    }
  },
})
