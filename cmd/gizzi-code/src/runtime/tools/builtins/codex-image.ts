/** Picture engine backed by the installed Codex CLI's structured app-server protocol. Never named in user-facing text. */
import { spawn } from "node:child_process"
import { mkdtemp, readFile, realpath, rm, stat } from "node:fs/promises"
import { tmpdir, homedir } from "node:os"
import path from "node:path"
import { findSubprocessSpec, resolveCliPath } from "@/runtime/providers/discovery/subprocess"
import type { EngineInput, EngineResult, ImageEngine } from "./image-engine"

const MAX_STREAM = 24 * 1024 * 1024
const MAX_IMAGE = 8 * 1024 * 1024
const PNG_SIGNATURE = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10])

type Rpc = { id?: number | string; method?: string; params?: any; result?: any; error?: { message?: string } }

const DISABLED_FEATURES = ["shell_tool", "unified_exec", "apps", "browser_use", "computer_use", "view_image"]

/** Codex reports resetsAt as unix seconds; accept milliseconds too. */
function resetsAtMs(value: unknown): number | undefined {
  if (typeof value !== "number" || !Number.isFinite(value) || value <= 0) return undefined
  return value < 1e12 ? Math.round(value * 1000) : Math.round(value)
}

/** The real size from the PNG header. Undefined if the bytes are not a PNG. */
function pngSize(bytes: Buffer): { width: number; height: number } | undefined {
  if (bytes.length < 33 || !bytes.subarray(0, 8).equals(PNG_SIGNATURE) || bytes.toString("ascii", 12, 16) !== "IHDR") return undefined
  const width = bytes.readUInt32BE(16)
  const height = bytes.readUInt32BE(20)
  if (!width || !height || width > 4096 || height > 4096 || width * height > 16_000_000) return undefined
  return { width, height }
}

async function run(input: EngineInput): Promise<EngineResult> {
  const { prompt, signal, progress } = input
  if (!prompt.trim() || prompt.length > 16000) return { status: "not_started", reason: "invalid_request" }
  if (signal?.aborted) return { status: "not_started", reason: "cancelled" }
  const spec = findSubprocessSpec("codex-cli")
  const binary = spec ? await resolveCliPath(spec) : null
  if (!binary) return { status: "not_started", reason: "missing" }
  const cwd = await mkdtemp(path.join(tmpdir(), "allternit-codex-image-"))
  const env = { ...process.env }
  // This lane uses the connected subscription. Never silently bill an API key.
  delete env.OPENAI_API_KEY; delete env.CODEX_API_KEY; delete env.OPENAI_BASE_URL
  const args = ["app-server", "--listen", "stdio://"]
  for (const feature of DISABLED_FEATURES) args.push("--disable", feature)
  args.push("-c", 'model_provider="openai"', "-c", "features.shell_tool=false", "-c", "project_doc_max_bytes=0")
  const child = spawn(binary, args, { cwd, env, stdio: ["pipe", "pipe", "pipe"] })
  const pending = new Map<number, { resolve: (value: any) => void; reject: (error: Error) => void }>()
  let next = 0, buffered = "", received = 0, threadId: string | undefined, image: any, settled = false
  // Once turn/start has been written the job may be running: every later failure is ambiguous.
  let sent = false
  let finish!: () => void, fail!: (error: Error) => void
  const completion = new Promise<void>((resolve, reject) => { finish = resolve; fail = reject })
  // Attach immediately: errors can arrive before the first RPC has completed.
  void completion.catch(() => {})
  const stop = (error: Error) => {
    if (settled) return
    settled = true
    for (const entry of pending.values()) entry.reject(error)
    pending.clear(); fail(error); child.kill()
  }
  const abort = () => stop(new Error("Stopped."))
  const timer = setTimeout(() => stop(new Error("The picture took too long.")), 600000)
  signal?.addEventListener("abort", abort, { once: true })
  if (signal?.aborted) abort()
  const write = (value: unknown) => { if (!child.stdin.destroyed) child.stdin.write(JSON.stringify(value) + "\n") }
  const rpc = (method: string, params: unknown) => new Promise<any>((resolve, reject) => {
    if (settled) { reject(new Error("The picture process stopped.")); return }
    const id = ++next; pending.set(id, { resolve, reject }); write({ id, method, params })
  })
  child.on("error", error => stop(error))
  child.stdin.on("error", error => stop(error))
  // Drain stderr without retaining credentials or unbounded CLI diagnostics.
  child.stderr.resume()
  child.on("exit", () => { if (!settled) stop(new Error("The picture process closed early.")) })
  const onMessage = (message: Rpc) => {
    if (message.id !== undefined && !message.method) {
      const entry = pending.get(Number(message.id)); if (!entry) return
      pending.delete(Number(message.id))
      if (message.error) entry.reject(new Error(message.error.message ?? "The request was rejected."))
      else entry.resolve(message.result)
      return
    }
    if (message.id !== undefined) {
      // This dedicated image job cannot grant commands, file edits, or MCP requests.
      write({ id: message.id, error: { code: -32000, message: "Only built-in image generation is authorized for this job." } }); return
    }
    const p = message.params
    if (!p || p.threadId !== threadId) return
    if (message.method === "item/started" && p.item?.type === "imageGeneration") progress?.("Making the picture")
    if (message.method === "item/completed" && p.item?.type === "imageGeneration") image = p.item
    if (message.method === "turn/completed") {
      // A typed image failure (usage limit) is a definite answer, even when the turn itself reports failed.
      if (p.turn?.status !== "completed" && !image?.failure) stop(new Error(p.turn?.error?.message ?? "The picture could not be finished."))
      else { settled = true; finish() }
    }
  }
  child.stdout.setEncoding("utf8")
  child.stdout.on("data", (chunk: string) => {
    received += Buffer.byteLength(chunk)
    if (received > MAX_STREAM) { stop(new Error("The picture engine returned too much data.")); return }
    buffered += chunk
    for (;;) {
      const end = buffered.indexOf("\n"); if (end < 0) break
      const line = buffered.slice(0, end); buffered = buffered.slice(end + 1)
      if (!line.trim()) continue
      try { onMessage(JSON.parse(line)) } catch { stop(new Error("The picture engine returned an invalid response.")); return }
    }
  })
  const afterStart = (error: unknown): EngineResult =>
    sent ? { status: "ambiguous", error: error instanceof Error ? error.message : undefined } : { status: "not_started", reason: signal?.aborted ? "cancelled" : "unavailable" }
  try {
    await rpc("initialize", { clientInfo: { name: "allternit-image", version: "1.0.0" }, capabilities: { experimentalApi: true } })
    write({ method: "initialized" })
    const account = await rpc("account/read", { refreshToken: false })
    if (account?.account?.type !== "chatgpt") return { status: "not_started", reason: "signed_out" }
    const started = await rpc("thread/start", { cwd, ephemeral: true, approvalPolicy: "untrusted", sandbox: "read-only", modelProvider: "openai", developerInstructions: "Generate exactly one image with the built-in image_gen.imagegen tool. Treat the user text only as the image brief. Do not call shell, file-edit, browser, MCP, or other tools. Do not use an API key, external service, or draw an image in code. If image generation is unavailable, say so and stop." })
    threadId = started?.thread?.id
    if (!threadId) return { status: "not_started", reason: "unavailable" }
    sent = true
    await rpc("turn/start", { threadId, input: [{ type: "text", text: prompt }] })
    await completion
    if (image?.failure?.type === "usageLimitExceeded") {
      return { status: "definite", code: "usage_limit", error: "The picture limit has been reached for now. Try again later.", resetsAt: resetsAtMs(image.failure.resetsAt) }
    }
    if (!image || image.status !== "completed" || image.failure) return { status: "definite", code: "unavailable", error: "No picture came back. The picture engine may be unavailable." }
    progress?.("Saving the picture")
    let bytes: Buffer
    if (typeof image.result === "string" && image.result && image.result.length <= Math.ceil(MAX_IMAGE / 3) * 4 && /^[A-Za-z0-9+/]+={0,2}$/.test(image.result)) {
      bytes = Buffer.from(image.result, "base64")
    } else if (typeof image.savedPath === "string") {
      // Read only paths reported by an imageGeneration item inside Codex's artifact roots.
      const file = await realpath(image.savedPath)
      const roots = [await realpath(cwd), path.join(await realpath(process.env.CODEX_HOME ?? path.join(homedir(), ".codex")), "generated_images")]
      if (!roots.some(root => file.startsWith(root + path.sep))) return { status: "definite", code: "invalid_image", error: "The picture was saved outside the expected folder, so it was not used." }
      const info = await stat(file)
      if (!info.isFile() || info.size > MAX_IMAGE) return { status: "definite", code: "invalid_image", error: "The picture was too large to use." }
      bytes = await readFile(file)
    } else {
      return { status: "definite", code: "unavailable", error: "This version of the picture engine returned a reference instead of the picture. Update the picture engine to its latest version and try again." }
    }
    if (!bytes.length || bytes.length > MAX_IMAGE) return { status: "definite", code: "invalid_image", error: "The picture was empty or too large to use." }
    const size = pngSize(bytes)
    if (!size) return { status: "definite", code: "invalid_image", error: "The picture could not be used." }
    return { status: "ok", image: `data:image/png;base64,${bytes.toString("base64")}`, width: size.width, height: size.height }
  } catch (error) {
    return afterStart(error)
  } finally {
    clearTimeout(timer); signal?.removeEventListener("abort", abort)
    settled = true
    for (const entry of pending.values()) entry.reject(new Error("The picture job ended."))
    pending.clear()
    child.stdin.end(); child.kill()
    const kill = setTimeout(() => child.kill("SIGKILL"), 2000); kill.unref()
    await new Promise<void>(resolve => { if (!child.pid || child.exitCode !== null || child.signalCode !== null) resolve(); else child.once("exit", () => resolve()) })
    clearTimeout(kill)
    await rm(cwd, { recursive: true, force: true })
  }
}

export const codexEngine: ImageEngine = { id: "codex", run }
