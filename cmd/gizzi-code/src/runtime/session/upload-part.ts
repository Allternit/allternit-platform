/**
 * Upload attachments reach gizzi as file parts whose url is the allternit-api
 * path `/api/v1/uploads/<id>` (iOS; web sends data: URLs). That is not a URL
 * anything downstream can open: the model provider cannot reach it and it
 * needs the caller's credential. gizzi talks to the same API (gateway base +
 * `gizzi login` / env token, on Desktop and on a cloud computer alike), so it
 * resolves the part here, once, to a data: URL that the model and `run_code`
 * both read.
 */
import { platformFetch } from "@/runtime/bots/platform-api"

export const UPLOAD_PATH_PREFIX = "/api/v1/uploads/"

/** True for a url that is a path rather than an absolute URL (not data:, http(s):, file:, …). */
export function isRelativeUrl(url: string): boolean {
  return url.startsWith("/") && !url.startsWith("//")
}

export type ResolvedUpload = { ok: true; url: string } | { ok: false; reason: string }

export async function resolveUploadUrl(url: string, mime: string, signal?: AbortSignal): Promise<ResolvedUpload> {
  if (!url.startsWith(UPLOAD_PATH_PREFIX) || url.includes("..")) {
    return { ok: false, reason: "not an upload path" }
  }
  try {
    const res = await platformFetch("GET", url, undefined, { signal, timeoutMs: 60_000, accept: "*/*" })
    if (!res.ok) return { ok: false, reason: `HTTP ${res.status}` }
    const bytes = Buffer.from(await res.arrayBuffer())
    return { ok: true, url: `data:${mime};base64,${bytes.toString("base64")}` }
  } catch (error) {
    return { ok: false, reason: error instanceof Error ? error.message : String(error) }
  }
}
