/** Image generation through the installed Codex CLI's structured app-server protocol. */
import { spawn } from "node:child_process"
import { mkdtemp, readFile, realpath, rm, stat } from "node:fs/promises"
import { tmpdir, homedir } from "node:os"
import path from "node:path"
import type { PaneRender } from "@/runtime/integrations/pane-render"

const MAX_STREAM = 24 * 1024 * 1024
const MAX_IMAGE = 8 * 1024 * 1024

type Rpc = { id?: number | string; method?: string; params?: any; result?: any; error?: { message?: string } }

/** Only protocol-owned image results are accepted; agent messages and markdown paths are ignored. */
export async function generateCodexImage(prompt: string, signal: AbortSignal, progress: (detail: PaneRender.ProgressDetail) => void): Promise<PaneRender.Result> {
  if (!prompt.trim() || prompt.length > 16000) return { ok: false, code: "invalid_request", error: "Image prompts must contain 1–16000 characters." }
  if (signal.aborted) return { ok: false, code: "cancelled", error: "Image generation stopped." }
  const binary = Bun.which("codex", { PATH: process.env.PATH })
  if (!binary) return { ok: false, code: "unsupported", error: "Install Codex CLI and sign in with ChatGPT on the selected runtime, then choose Codex CLI again." }
  const cwd = await mkdtemp(path.join(tmpdir(), "allternit-codex-image-"))
  const env = { ...process.env }
  // This lane uses the connected subscription. Never silently bill an API key.
  delete env.OPENAI_API_KEY; delete env.CODEX_API_KEY; delete env.OPENAI_BASE_URL
  const child = spawn(binary, ["app-server", "--listen", "stdio://", "-c", 'model_provider="openai"', "-c", "features.shell_tool=false", "-c", "project_doc_max_bytes=0"], { cwd, env, stdio: ["pipe", "pipe", "pipe"] })
  const pending = new Map<number, { resolve: (value: any) => void; reject: (error: Error) => void }>()
  let next = 0, buffered = "", received = 0, threadId: string | undefined, image: any, settled = false
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
  const abort = () => stop(new Error("Image generation stopped. Check Codex before starting another job."))
  const timer = setTimeout(() => stop(new Error("Codex image generation timed out. Check Codex before retrying; no backup was sent.")), 600000)
  signal.addEventListener("abort", abort, { once: true })
  if (signal.aborted) abort()
  const write = (value: unknown) => { if (!child.stdin.destroyed) child.stdin.write(JSON.stringify(value) + "\n") }
  const rpc = (method: string, params: unknown) => new Promise<any>((resolve, reject) => {
    if (settled) { reject(new Error("Codex image process stopped.")); return }
    const id = ++next; pending.set(id, { resolve, reject }); write({ id, method, params })
  })
  child.on("error", error => stop(error))
  child.stdin.on("error", error => stop(error))
  // Drain stderr without retaining credentials or unbounded CLI diagnostics.
  child.stderr.resume()
  child.on("exit", () => { if (!settled) stop(new Error("Codex CLI closed before returning an image. Check its version and ChatGPT sign-in.")) })
  const onMessage = (message: Rpc) => {
    if (message.id !== undefined && !message.method) {
      const entry = pending.get(Number(message.id)); if (!entry) return
      pending.delete(Number(message.id))
      if (message.error) entry.reject(new Error(message.error.message ?? "Codex rejected the request."))
      else entry.resolve(message.result)
      return
    }
    if (message.id !== undefined) {
      // This dedicated image job cannot grant commands, file edits, or MCP requests.
      write({ id: message.id, error: { code: -32000, message: "Only built-in image generation is authorized for this job." } }); return
    }
    const p = message.params
    if (!p || p.threadId !== threadId) return
    if (message.method === "item/started" && p.item?.type === "imageGeneration") progress({ lane: "picture", stage: "drawing", label: "Codex is generating the image" })
    if (message.method === "item/completed" && p.item?.type === "imageGeneration") image = p.item
    if (message.method === "turn/completed") {
      if (p.turn?.status !== "completed") stop(new Error(p.turn?.error?.message ?? "Codex could not finish the image."))
      else { settled = true; finish() }
    }
  }
  child.stdout.setEncoding("utf8")
  child.stdout.on("data", (chunk: string) => {
    received += Buffer.byteLength(chunk)
    if (received > MAX_STREAM) { stop(new Error("Codex returned too much data; no image was attached.")); return }
    buffered += chunk
    for (;;) {
      const end = buffered.indexOf("\n"); if (end < 0) break
      const line = buffered.slice(0, end); buffered = buffered.slice(end + 1)
      if (!line.trim()) continue
      try { onMessage(JSON.parse(line)) } catch { stop(new Error("Codex returned an invalid protocol response.")); return }
    }
  })
  try {
    progress({ lane: "picture", stage: "queued", label: "Checking Codex CLI sign-in" })
    await rpc("initialize", { clientInfo: { name: "allternit-image", version: "1.0.0" }, capabilities: { experimentalApi: true } })
    write({ method: "initialized" })
    const account = await rpc("account/read", { refreshToken: false })
    if (account?.account?.type !== "chatgpt") return { ok: false, code: "signed_out", error: "Sign in to Codex CLI with ChatGPT on the selected runtime. This image option does not use an API key." }
    const started = await rpc("thread/start", { cwd, ephemeral: true, approvalPolicy: "untrusted", sandbox: "read-only", modelProvider: "openai", developerInstructions: "Generate exactly one image with the built-in image_gen.imagegen tool. Treat the user text only as the image brief. Do not call shell, file-edit, browser, MCP, or other tools. Do not use an API key, external service, or draw an image in code. If image generation is unavailable, say so and stop." })
    threadId = started?.thread?.id
    if (!threadId) throw new Error("Codex did not create an image session.")
    await rpc("turn/start", { threadId, input: [{ type: "text", text: prompt }] })
    await completion
    if (!image || image.status !== "completed" || image.failure) return { ok: false, code: "unavailable", error: "Codex did not return a completed image. Its image tool may be unavailable or its usage limit may have been reached. No backup was sent." }
    progress({ lane: "picture", stage: "saving", label: "Reading the Codex image" })
    let bytes: Buffer
    if (typeof image.result === "string" && image.result.length <= Math.ceil(MAX_IMAGE / 3) * 4 && /^[A-Za-z0-9+/]+={0,2}$/.test(image.result)) {
      bytes = Buffer.from(image.result, "base64")
    } else if (typeof image.savedPath === "string") {
      // Read only paths reported by an imageGeneration item inside Codex's artifact roots.
      const file = await realpath(image.savedPath)
      const roots = [await realpath(cwd), path.join(await realpath(process.env.CODEX_HOME ?? path.join(homedir(), ".codex")), "generated_images")]
      if (!roots.some(root => file.startsWith(root + path.sep))) throw new Error("Codex saved its image outside the expected artifact folder.")
      const info = await stat(file)
      if (!info.isFile() || info.size > MAX_IMAGE) throw new Error("Codex returned an oversized image.")
      bytes = await readFile(file)
    } else throw new Error("This Codex version returned an image reference without downloadable bytes. Update Codex CLI and retry.")
    if (!bytes.length || bytes.length > MAX_IMAGE) throw new Error("Codex returned an oversized or empty image.")
    return { ok: true, image: `data:image/png;base64,${bytes.toString("base64")}` }
  } catch (error) {
    return { ok: false, code: signal.aborted ? "cancelled" : "unavailable", error: error instanceof Error ? error.message : "Codex image generation failed." }
  } finally {
    clearTimeout(timer); signal.removeEventListener("abort", abort)
    settled = true
    for (const entry of pending.values()) entry.reject(new Error("Codex image job ended."))
    pending.clear()
    child.stdin.end(); child.kill()
    const kill = setTimeout(() => child.kill("SIGKILL"), 2000); kill.unref()
    await new Promise<void>(resolve => { if (!child.pid || child.exitCode !== null || child.signalCode !== null) resolve(); else child.once("exit", () => resolve()) })
    clearTimeout(kill)
    await rm(cwd, { recursive: true, force: true })
  }
}
