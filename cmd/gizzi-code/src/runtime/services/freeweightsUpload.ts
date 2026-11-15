// Publish a local model folder to Freeweights (freeweights.allternit.com) through its
// Hugging Face compatible upload API: create repo, preupload, Git LFS batch, PUT (or
// multipart), verify, then one NDJSON commit. Needs an Allternit key (alt_...) with the
// freeweights.write scope. Caps are read from GET /api/fw/v1/limits, never written here.
import * as fs from "fs"
import * as path from "path"
import { createHash } from "crypto"

export const FREEWEIGHTS_ORIGIN = process.env.FREEWEIGHTS_ORIGIN?.replace(/\/+$/, "") || "https://freeweights.allternit.com"

export class FreeweightsError extends Error {
  constructor(message: string, readonly status?: number, readonly code?: string) {
    super(message)
  }
}

/** The key used for uploads: FREEWEIGHTS_TOKEN, else ALLTERNIT_API_KEY when it is an alt_ key. */
export function freeweightsKey(env: NodeJS.ProcessEnv = process.env): string | undefined {
  for (const v of [env.FREEWEIGHTS_TOKEN, env.ALLTERNIT_API_KEY, env.HF_TOKEN]) {
    const t = v?.trim()
    if (t?.startsWith("alt_")) return t
  }
  return undefined
}

export function parseRepoId(id: string): { namespace: string; name: string } {
  const m = /^([A-Za-z0-9][A-Za-z0-9._-]{0,95})\/([A-Za-z0-9._-]{1,96})$/.exec(id.trim())
  if (!m) throw new FreeweightsError(`"${id}" is not a repo id. Use <handle>/<name>.`)
  return { namespace: m[1], name: m[2] }
}

export interface FreeweightsLimits {
  uploads_open?: boolean
  site_cap_bytes?: number
  user_cap_bytes?: number
  file_max_bytes?: number
  repos_per_user?: number
  files_per_commit?: number
  used_bytes?: number
  [k: string]: unknown
}

/** GET /api/fw/v1/limits. Accepts either a flat object or { limits: {...}, usage: {...} }. */
export async function fetchLimits(key?: string, fetcher: typeof fetch = fetch): Promise<FreeweightsLimits | undefined> {
  try {
    const resp = await fetcher(`${FREEWEIGHTS_ORIGIN}/api/fw/v1/limits`, {
      headers: { Accept: "application/json", ...(key ? { Authorization: `Bearer ${key}` } : {}) },
      signal: AbortSignal.timeout(8000),
    })
    if (!resp.ok) return undefined
    const body = (await resp.json()) as Record<string, any>
    const limits = (body.limits && typeof body.limits === "object" ? body.limits : body) as FreeweightsLimits
    const usage = body.usage && typeof body.usage === "object" ? body.usage : {}
    return { ...limits, used_bytes: Number(usage.used_bytes ?? usage.user_bytes ?? limits.used_bytes) || limits.used_bytes }
  } catch {
    return undefined
  }
}

/** One line for `gizzi usage`. Undefined when nothing useful came back. */
export function storageLine(l: FreeweightsLimits | undefined): string | undefined {
  if (!l || typeof l.user_cap_bytes !== "number") return undefined
  const gb = (n: number) => (n >= 1e9 ? `${(n / 1e9).toFixed(2)} GB` : `${(n / 1e6).toFixed(0)} MB`)
  const used = typeof l.used_bytes === "number" ? `${gb(l.used_bytes)} of ` : "limit "
  const paused = l.uploads_open === false ? " (uploads are paused)" : ""
  return `Freeweights storage: ${used}${gb(l.user_cap_bytes)}${paused}`
}

export async function sha256File(file: string): Promise<string> {
  const h = createHash("sha256")
  for await (const chunk of fs.createReadStream(file)) h.update(chunk as Buffer)
  return h.digest("hex")
}

export function listFiles(root: string): { path: string; abs: string; size: number }[] {
  const out: { path: string; abs: string; size: number }[] = []
  const walk = (dir: string) => {
    for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
      if (e.name === ".git" || e.name === ".DS_Store" || e.isSymbolicLink()) continue
      const abs = path.join(dir, e.name)
      if (e.isDirectory()) walk(abs)
      else if (e.isFile()) out.push({ path: path.relative(root, abs).split(path.sep).join("/"), abs, size: fs.statSync(abs).size })
    }
  }
  const st = fs.statSync(root)
  if (st.isFile()) return [{ path: path.basename(root), abs: root, size: st.size }]
  walk(root)
  return out.sort((a, b) => a.path.localeCompare(b.path))
}

export function chunk<T>(items: T[], n: number): T[][] {
  const size = Math.max(1, Math.floor(n) || 1)
  const out: T[][] = []
  for (let i = 0; i < items.length; i += size) out.push(items.slice(i, i + size))
  return out
}

/** NDJSON body of one commit. */
export function commitBody(
  summary: string,
  parent: string | undefined,
  ops: ({ kind: "file"; path: string; base64: string } | { kind: "lfs"; path: string; oid: string; size: number })[],
): string {
  const lines: unknown[] = [{ key: "header", value: { summary, ...(parent ? { parentCommit: parent } : {}) } }]
  for (const o of ops) {
    lines.push(
      o.kind === "file"
        ? { key: "file", value: { path: o.path, content: o.base64, encoding: "base64" } }
        : { key: "lfsFile", value: { path: o.path, algo: "sha256", oid: o.oid, size: o.size } },
    )
  }
  return lines.map((l) => JSON.stringify(l)).join("\n") + "\n"
}

const PLAIN: Record<string, string> = {
  uploads_paused: "Uploads are paused right now. Try again later.",
  site_full: "Freeweights storage is full for now.",
  file_too_large: "A file is over the per-file limit. See `gizzi usage` for the current limits.",
  file_type_not_allowed: "Executables and archives cannot be uploaded.",
  hf_token_rejected: "Use an Allternit key (alt_...), not a Hugging Face token.",
}

async function fail(resp: Response, what: string): Promise<never> {
  let code: string | undefined
  let msg = ""
  try {
    const b = (await resp.json()) as any
    code = typeof b?.error === "string" ? b.error : b?.error?.code ?? b?.code
    msg = (typeof b?.error === "object" ? b.error?.message : undefined) ?? b?.message ?? ""
  } catch {}
  const hint = (code && PLAIN[code]) || (resp.status === 401 ? "The key was not accepted. Create one with the freeweights.write scope in the console." : resp.status === 402 ? "Private repos need a plan that includes private storage." : "")
  throw new FreeweightsError(`${what} failed (HTTP ${resp.status}${code ? `, ${code}` : ""})${hint || msg ? `: ${hint || msg}` : ""}`, resp.status, code)
}

export interface PublishOptions {
  key: string
  summary?: string
  log?: (line: string) => void
  fetcher?: typeof fetch
}

export interface PublishResult {
  repoUrl: string
  commitUrls: string[]
  files: number
}

export async function publishModel(dir: string, repoId: string, opts: PublishOptions): Promise<PublishResult> {
  const f = opts.fetcher ?? fetch
  const log = opts.log ?? (() => {})
  const { namespace, name } = parseRepoId(repoId)
  const auth = { Authorization: `Bearer ${opts.key}` }
  const base = `${FREEWEIGHTS_ORIGIN}/api/models/${namespace}/${name}`
  const files = listFiles(dir)
  if (files.length === 0) throw new FreeweightsError(`${dir} has no files to upload.`)

  const limits = await fetchLimits(opts.key, f)
  if (limits?.uploads_open === false) throw new FreeweightsError(PLAIN.uploads_paused)
  if (limits?.file_max_bytes) {
    const big = files.find((x) => x.size > limits.file_max_bytes!)
    if (big) throw new FreeweightsError(`${big.path} is over the per-file limit (${(limits.file_max_bytes / 1e6).toFixed(0)} MB).`)
  }

  // 1. create (409 = exists, fine)
  const created = await f(`${FREEWEIGHTS_ORIGIN}/api/repos/create`, {
    method: "POST",
    headers: { ...auth, "Content-Type": "application/json" },
    body: JSON.stringify({ name, organization: namespace, type: "model" }),
  })
  if (!created.ok && created.status !== 409) await fail(created, "Creating the repo")
  log(created.status === 409 ? `Repo ${repoId} exists` : `Created ${repoId}`)

  // 2. preupload: which files go through LFS
  const modes = new Map<string, "lfs" | "regular">()
  for (const part of chunk(files, 1000)) {
    const samples = part.map((x) => {
      const fd = fs.openSync(x.abs, "r")
      const buf = Buffer.alloc(Math.min(512, x.size))
      fs.readSync(fd, buf, 0, buf.length, 0)
      fs.closeSync(fd)
      return { path: x.path, sample: buf.toString("base64"), size: x.size }
    })
    const r = await f(`${base}/preupload/main`, {
      method: "POST",
      headers: { ...auth, "Content-Type": "application/json" },
      body: JSON.stringify({ files: samples }),
    })
    if (!r.ok) await fail(r, "Checking the files")
    const body = (await r.json()) as { files: { path: string; uploadMode: "lfs" | "regular"; shouldIgnore: boolean }[] }
    for (const x of body.files) if (!x.shouldIgnore) modes.set(x.path, x.uploadMode)
  }
  const todo = files.filter((x) => modes.has(x.path))

  // 3. LFS: hash, batch, PUT, verify
  const ops: Parameters<typeof commitBody>[2] = []
  const lfs = todo.filter((x) => modes.get(x.path) === "lfs")
  for (const x of todo) {
    if (modes.get(x.path) === "regular") ops.push({ kind: "file", path: x.path, base64: fs.readFileSync(x.abs).toString("base64") })
  }
  for (const x of lfs) {
    log(`Hashing ${x.path}`)
    const oid = await sha256File(x.abs)
    const batch = await f(`${FREEWEIGHTS_ORIGIN}/${namespace}/${name}.git/info/lfs/objects/batch`, {
      method: "POST",
      headers: { ...auth, "Content-Type": "application/vnd.git-lfs+json", Accept: "application/vnd.git-lfs+json" },
      body: JSON.stringify({ operation: "upload", transfers: ["basic", "multipart"], objects: [{ oid, size: x.size }], hash_algo: "sha256" }),
    })
    if (!batch.ok) await fail(batch, `Preparing ${x.path}`)
    const obj = ((await batch.json()) as any).objects?.[0]
    if (obj?.error) {
      const c = Number(obj.error.code)
      throw new FreeweightsError(c === 507 ? "Your Freeweights storage limit is reached. Delete a repo or see `gizzi usage`." : `${x.path}: ${obj.error.message ?? "refused"}`, c)
    }
    const up = obj?.actions?.upload
    if (up) {
      log(`Uploading ${x.path} (${(x.size / 1e6).toFixed(1)} MB)`)
      await putObject(f, x.abs, x.size, up, auth)
      const ver = obj.actions.verify
      if (ver?.href) {
        const v = await f(ver.href, {
          method: "POST",
          headers: { ...auth, ...(ver.header ?? {}), "Content-Type": "application/vnd.git-lfs+json" },
          body: JSON.stringify({ oid, size: x.size }),
        })
        if (!v.ok) await fail(v, `Verifying ${x.path}`)
      }
    } else log(`${x.path} already stored`)
    ops.push({ kind: "lfs", path: x.path, oid, size: x.size })
  }

  // 4. commit, split by the current files-per-commit limit
  const commitUrls: string[] = []
  for (const [i, group] of chunk(ops, limits?.files_per_commit ?? 100).entries()) {
    const summary = opts.summary ?? "Upload from gizzi models publish"
    const r = await f(`${base}/commit/main`, {
      method: "POST",
      headers: { ...auth, "Content-Type": "application/x-ndjson" },
      body: commitBody(i === 0 ? summary : `${summary} (part ${i + 1})`, undefined, group),
    })
    if (!r.ok) await fail(r, "Committing")
    const b = (await r.json()) as { commitUrl?: string }
    if (b.commitUrl) commitUrls.push(b.commitUrl)
  }
  return { repoUrl: `${FREEWEIGHTS_ORIGIN}/${namespace}/${name}`, commitUrls, files: ops.length }
}

async function withRetry<T>(fn: () => Promise<T>, tries = 3): Promise<T> {
  let last: unknown
  for (let i = 0; i < tries; i++) {
    try {
      return await fn()
    } catch (e) {
      last = e
      await new Promise((r) => setTimeout(r, 500 * 2 ** i))
    }
  }
  throw last
}

async function putObject(
  f: typeof fetch,
  file: string,
  size: number,
  up: { href: string; header?: Record<string, string> },
  auth: Record<string, string>,
) {
  const header = up.header ?? {}
  const chunkSize = Number(header.chunk_size)
  if (!chunkSize) {
    // basic: presigned PUT, no Authorization header
    await withRetry(async () => {
      const r = await f(up.href, { method: "PUT", headers: { "Content-Length": String(size) }, body: Bun.file(file) })
      if (!r.ok) throw new FreeweightsError(`Upload failed (HTTP ${r.status}).`, r.status)
    })
    return
  }
  const parts: { partNumber: number; etag: string }[] = []
  const keys = Object.keys(header).filter((k) => /^\d+$/.test(k)).sort()
  for (const k of keys) {
    const n = Number(k)
    const start = (n - 1) * chunkSize
    const end = Math.min(size, start + chunkSize)
    const etag = await withRetry(async () => {
      const r = await f(header[k], { method: "PUT", body: Bun.file(file).slice(start, end) })
      if (!r.ok) throw new FreeweightsError(`Part ${n} failed (HTTP ${r.status}).`, r.status)
      const e = r.headers.get("etag")
      if (!e) throw new FreeweightsError(`Part ${n} came back without an ETag.`)
      return e
    })
    parts.push({ partNumber: n, etag })
  }
  const done = await f(up.href, {
    method: "POST",
    headers: { ...auth, "Content-Type": "application/vnd.git-lfs+json" },
    body: JSON.stringify({ oid: await sha256File(file), parts }),
  })
  if (!done.ok) await fail(done, "Finishing the upload")
}

/** Community repo check before a pull: { found, live }. Uses the HF-compatible model info route. */
export async function communityRepoState(repo: string, f: typeof fetch = fetch): Promise<{ found: boolean; live: boolean; message?: string }> {
  try {
    const { namespace, name } = parseRepoId(repo)
    const r = await f(`${FREEWEIGHTS_ORIGIN}/api/models/${namespace}/${name}`, { headers: { Accept: "application/json" }, signal: AbortSignal.timeout(8000) })
    if (r.status === 404) return { found: false, live: false }
    if (r.status === 451) return { found: true, live: false, message: "This repo was removed." }
    if (!r.ok) return { found: false, live: false }
    const b = (await r.json()) as any
    const state = b?.state ?? b?.freeweights?.state
    return { found: true, live: state === undefined || state === "live", message: state && state !== "live" ? `This repo is ${state}; files are served after the scan finishes.` : undefined }
  } catch {
    return { found: false, live: false }
  }
}
