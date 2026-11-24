/**
 * The agent handing something to its own user: a message shown in the thread,
 * optional files, and an optional push.
 *
 * Shared core for `send_user_message` (runtime builtin and Ink adapter); both
 * share tools/builtins/send-user-message.txt. The caller supplies the folders
 * a file may come from (`ctx.roots`); each adapter computes them the way its
 * own file tools do.
 *
 * - The message itself is the tool result: clients render `text` and `files`
 *   from the result's metadata as a message card. Nothing is sent as a data URL.
 * - Files: each must resolve (symlinks followed) inside a root, must not match
 *   a credential pattern (the `read` tool's list), must be a file, and at most
 *   MAX_FILE_BYTES. Each is copied to the Files drive's Agent Output folder
 *   (allternit-api `user/files`). When the drive is off or the API can't be
 *   reached the file stays listed by path, without an id.
 * - Push: never by default. `push` asks for `agent.message.sent` at level
 *   normal (quiet hours apply), `needs_you` for `thread.needs_user` at normal
 *   urgency (allternit-api `user/notify`). A push is "requested", never
 *   "delivered": the result never claims the phone got it.
 *
 * Free of runtime state and UI imports so the Ink adapter and bun tests can
 * use it; tests stub only `fetch`.
 */
import fs from "fs/promises"
import path from "path"
import z from "zod/v4"
import { AppRoutingError, notifyUser, sendUserFile } from "@/runtime/bots/app-bot-routing"
import { isCredentialPath } from "@/runtime/tools/guard/permission/credential-paths"

export namespace UserMessage {
  export const MAX_TEXT_CHARS = 4000
  export const MAX_ATTACHMENTS = 5
  export const MAX_CAPTION_CHARS = 200
  /** Same as allternit-api `agent_user_tools::MAX_FILE_BYTES`. */
  export const MAX_FILE_BYTES = 25 * 1024 * 1024
  /** The push preview (allternit-api clips to the same). */
  export const MAX_PREVIEW_CHARS = 200

  export type Display = "render" | "attach"
  export type Notify = "none" | "push" | "needs_you"
  export type Push = "sent" | "not_requested" | "rate_limited" | "unpaired" | "unavailable"

  /** Why a file has no Files-drive id. */
  export type NotSaved = "drive_off" | "unavailable" | "failed"

  export type File = {
    /** Files drive id (Agent Output); missing when the file was not saved there. */
    id?: string
    name: string
    size: number
    display: Display
    /** The real path on this machine (symlinks resolved). */
    path: string
    caption?: string
    not_saved?: NotSaved
  }

  export type Result = { text: string; files: File[]; push: Push }

  /** What clients render (the tool's metadata). */
  export type Metadata = Result

  /** The HTTP edge (allternit-api). Defaults to the app-bot-routing client. */
  export interface Api {
    sendFile(sessionID: string, input: { path: string; caption?: string }): Promise<{ id: string; name: string; size: number }>
    notify(
      sessionID: string,
      input: { kind: "message" | "question"; level: "normal" | "needs_you"; preview: string; ref?: string },
    ): Promise<{ pushed: boolean; reason?: string }>
  }

  export interface Context {
    /** Folders a file may come from: the project, the session's working folder, the scratchpad. */
    roots: string[]
    /** Relative attachment paths resolve against this (default: the first root). */
    cwd?: string
    /** Sent with the push so the app can open the right message. */
    ref?: string
    api?: Api
  }

  const defaultApi: Api = { sendFile: sendUserFile, notify: notifyUser }

  export const attachment = z.object({
    path: z.string().min(1).describe("Path to the file (absolute, or relative to the working folder)"),
    caption: z.string().max(MAX_CAPTION_CHARS).optional().describe("A short line shown with the file"),
    display: z
      .enum(["render", "attach"])
      .optional()
      .describe("render: show it inline (images, PDF, Markdown, HTML render by default); attach: show it as a file to open"),
  })

  export const parameters = z.object({
    text: z.string().min(1).max(MAX_TEXT_CHARS).describe("The message for the user. Markdown is fine."),
    attachments: z
      .array(attachment)
      .max(MAX_ATTACHMENTS)
      .optional()
      .describe(`Up to ${MAX_ATTACHMENTS} finished files for the user`),
    notify: z
      .enum(["none", "push", "needs_you"])
      .optional()
      .describe("none (default): only shown in the thread; push: also notify the user's devices; needs_you: notify that they must decide or act"),
  })

  export type Params = z.infer<typeof parameters>

  const RENDER_EXTENSIONS = new Set([
    ".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".avif", ".heic",
    ".pdf", ".md", ".markdown", ".html", ".htm",
  ])

  /** render for images, PDF, Markdown and HTML; attach for anything else. */
  export function defaultDisplay(filepath: string): Display {
    return RENDER_EXTENSIONS.has(path.extname(filepath).toLowerCase()) ? "render" : "attach"
  }

  function inside(root: string, child: string): boolean {
    const rel = path.relative(root, child)
    return rel !== "" && !path.isAbsolute(rel) && rel.split(path.sep)[0] !== ".."
  }

  async function real(p: string): Promise<string> {
    return fs.realpath(p).catch(() => path.resolve(p))
  }

  type Checked = { given: string; real: string; size: number; display: Display; caption?: string }

  /**
   * Resolve and check one attachment. Throws a sentence the model can act on.
   * Exported for the adapters' input validation.
   */
  export async function checkAttachment(input: z.infer<typeof attachment>, ctx: Context): Promise<Checked> {
    const cwd = ctx.cwd ?? ctx.roots[0] ?? process.cwd()
    const given = path.isAbsolute(input.path) ? path.normalize(input.path) : path.resolve(cwd, input.path)
    if (isCredentialPath(given)) {
      throw new Error(`${input.path} looks like a credential or git control file, so it can't be sent. Send a finished deliverable instead.`)
    }
    let resolved: string
    try {
      resolved = await fs.realpath(given)
    } catch {
      throw new Error(`There's no file at ${given}.`)
    }
    if (isCredentialPath(resolved)) {
      throw new Error(`${input.path} points to a credential or git control file, so it can't be sent.`)
    }
    const roots = await Promise.all(ctx.roots.filter(Boolean).map(real))
    if (!roots.some((root) => inside(root, resolved))) {
      throw new Error(
        `${input.path} is outside the project, the session's working folder and the scratchpad (symlinks are followed), so it can't be sent. Copy the deliverable into the project or the scratchpad first.`,
      )
    }
    const stat = await fs.stat(resolved)
    if (stat.isDirectory()) throw new Error(`${input.path} is a folder. Send files one at a time.`)
    if (!stat.isFile()) throw new Error(`${input.path} is not a regular file.`)
    if (stat.size > MAX_FILE_BYTES) {
      throw new Error(`${input.path} is ${(stat.size / (1024 * 1024)).toFixed(1)} MB; files can be at most ${MAX_FILE_BYTES / (1024 * 1024)} MB.`)
    }
    const caption = input.caption?.trim().slice(0, MAX_CAPTION_CHARS) || undefined
    return { given, real: resolved, size: stat.size, display: input.display ?? defaultDisplay(resolved), caption }
  }

  function notSavedReason(error: unknown): NotSaved {
    if (error instanceof AppRoutingError) {
      if (error.code === "drive_off" || error.status === 409) return "drive_off"
      if (error.status === 401 || error.status === 403) return "unavailable"
      return "failed"
    }
    // Network error, timeout, no API on this machine.
    return "unavailable"
  }

  function pushOf(reason: string | undefined, pushed: boolean): Push {
    if (pushed) return "sent"
    if (reason === "rate_limited") return "rate_limited"
    if (reason === "unpaired") return "unpaired"
    if (reason === "incognito") return "not_requested"
    return "unavailable"
  }

  /** The push preview: the text on one line, clipped. */
  export function preview(text: string): string {
    const line = text.replace(/\s+/g, " ").trim()
    return line.length > MAX_PREVIEW_CHARS ? line.slice(0, MAX_PREVIEW_CHARS - 1).trimEnd() + "…" : line
  }

  /** Short title for the thread: the first line, clipped. */
  export function title(text: string): string {
    const first = text.trim().split("\n", 1)[0]!.replace(/^#+\s*/, "").trim()
    return first.length > 80 ? first.slice(0, 79).trimEnd() + "…" : first || "Message"
  }

  /** The one line the model reads back. Says "requested" for a push, never "delivered to the phone". */
  export function summary(result: Result, notify: Notify = "none"): string {
    const n = result.files.length
    const unsaved = result.files.filter((f) => !f.id)
    let line = `Delivered to the user in this thread${n ? ` (${n} file${n === 1 ? "" : "s"})` : ""}.`
    if (unsaved.length) {
      const why = unsaved.some((f) => f.not_saved === "drive_off")
        ? "the Files drive is off on this computer"
        : "the Allternit API couldn't be reached"
      line += ` ${unsaved.length} not saved to Agent Output (${why}); the user sees ${unsaved.length === 1 ? "its" : "their"} path.`
    }
    const push: Record<Push, string> = {
      sent: "Push: requested.",
      not_requested: notify === "none" ? "Push: not requested." : "Push: not_requested (an incognito chat never leaves this computer).",
      rate_limited: "Push: rate_limited (too many pushes recently). The message is still in the thread; don't retry the push.",
      unpaired: "Push: unpaired (this runtime isn't linked to the user's cloud account, so nothing reaches their devices).",
      unavailable: "Push: unavailable (no connection to the user's Allternit account).",
    }
    return `${line} ${push[result.push]}`
  }

  /**
   * Check every attachment first (nothing is sent when one is refused), then
   * save the files and ask for the push.
   */
  export async function sendUserMessage(input: {
    sessionID: string
    text: string
    attachments?: Params["attachments"]
    notify?: Notify
    ctx: Context
  }): Promise<Result> {
    const text = input.text.trim()
    if (!text) throw new Error("text is empty.")
    if (text.length > MAX_TEXT_CHARS) throw new Error(`text is ${text.length} characters; the limit is ${MAX_TEXT_CHARS}.`)
    const attachments = input.attachments ?? []
    if (attachments.length > MAX_ATTACHMENTS) {
      throw new Error(`${attachments.length} attachments; the limit is ${MAX_ATTACHMENTS}.`)
    }
    for (const a of attachments) {
      if (a.caption && a.caption.length > MAX_CAPTION_CHARS) {
        throw new Error(`The caption for ${a.path} is longer than ${MAX_CAPTION_CHARS} characters.`)
      }
    }
    const api = input.ctx.api ?? defaultApi
    const checked: Checked[] = []
    for (const a of attachments) checked.push(await checkAttachment(a, input.ctx))

    const files: File[] = []
    for (const c of checked) {
      const file: File = { name: path.basename(c.given), size: c.size, display: c.display, path: c.real }
      if (c.caption) file.caption = c.caption
      try {
        const saved = await api.sendFile(input.sessionID, { path: c.real, caption: c.caption })
        file.id = saved.id
      } catch (error) {
        file.not_saved = notSavedReason(error)
      }
      files.push(file)
    }

    const notify = input.notify ?? "none"
    let push: Push = "not_requested"
    if (notify !== "none") {
      try {
        const r = await api.notify(input.sessionID, {
          kind: "message",
          level: notify === "needs_you" ? "needs_you" : "normal",
          preview: preview(text),
          ref: input.ctx.ref,
        })
        push = pushOf(r.reason, r.pushed === true)
      } catch {
        // No platform credential, API not running, timeout.
        push = "unavailable"
      }
    }
    return { text, files, push }
  }
}

export const sendUserMessage = UserMessage.sendUserMessage
