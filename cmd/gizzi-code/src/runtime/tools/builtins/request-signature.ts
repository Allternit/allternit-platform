/**
 * request_signature: asks the user to sign a PDF. Non-blocking: uploads the
 * document, files a sign request (cloud-api /api/v1/sign-requests) and returns
 * at once with the id and link; the user signs on Desktop or phone. There is no
 * sign/approve/decline parameter, and the status read never returns the image.
 * Cloud-api create accepts a Clerk session or an API key, not a runtime device
 * token, so ALLTERNIT_API_TOKEN is required.
 */
import z from "zod/v4"
import { createHash } from "node:crypto"
import { readFile, stat } from "node:fs/promises"
import path from "node:path"
import { Tool } from "@/runtime/tools/builtins/tool"
import { platformFetch, PlatformApiError } from "@/runtime/bots/platform-api"
import DESCRIPTION from "@/runtime/tools/builtins/request-signature.txt"

const MAX_BYTES = 20 * 1024 * 1024
const SIGN_BASE = "https://ai.allternit.com/sign"

export const parameters = z.object({
  id: z.string().trim().min(1).optional().describe("Status mode: the request id returned earlier. Omit everything else."),
  title: z.string().trim().min(1).max(200).optional().describe("What the user is signing, in a few words"),
  summary: z.string().trim().max(1000).optional().describe("One or two sentences on what it is and why"),
  details: z
    .array(z.object({ label: z.string().trim().min(1).max(80), value: z.string().trim().min(1).max(500) }))
    .max(8)
    .optional()
    .describe("Up to 8 label/value rows shown above the document"),
  document_path: z.string().trim().min(1).optional().describe("Path to the PDF to sign (max 20 MB)"),
  send_to: z
    .object({ name: z.string().trim().max(200).optional(), email: z.string().trim().email() })
    .optional()
    .describe("Where to send the signed copy once the user has signed"),
})

export type Metadata = {
  id: string
  status: string
  sign_url: string
  title?: string
  signer_name?: string
  resolved_at?: string
}

type SignRequest = {
  id: string
  status: string
  title?: string
  signer_name?: string
  resolved_at?: string
}

function signUrl(id: string) {
  return `${SIGN_BASE}/${id}`
}

async function errorOf(res: Response, what: string): Promise<Error> {
  const text = await res.text().catch(() => "")
  let message = text
  try {
    const parsed = JSON.parse(text) as { message?: string; error?: string; code?: string }
    message = parsed.message || parsed.error || parsed.code || text
  } catch {}
  if (res.status === 401 || res.status === 403)
    return new Error(
      `${what}: Allternit rejected the credential (HTTP ${res.status}). Ask the user to check ALLTERNIT_API_TOKEN (an API key from their signed-in account).`,
    )
  return new PlatformApiError(res.status, `${what}: ${message || `HTTP ${res.status}`}`)
}

async function upload(file: string, bytes: Buffer): Promise<string> {
  const body = { name: path.basename(file), contentType: "application/pdf", bytes: bytes.length }
  const begin = await platformFetch("POST", "/api/v1/files/uploads", body, { timeoutMs: 30_000 })
  if (!begin.ok) throw await errorOf(begin, "Could not start the upload")
  const { fileId, putUrl } = (await begin.json()) as { fileId: string; putUrl: string }
  const put = await fetch(putUrl, {
    method: "PUT",
    headers: { "Content-Type": "application/pdf" },
    body: new Uint8Array(bytes),
    signal: AbortSignal.timeout(120_000),
  })
  if (!put.ok) throw new Error(`Could not upload the document (HTTP ${put.status}).`)
  const done = await platformFetch("POST", `/api/v1/files/${fileId}/complete`, body, { timeoutMs: 30_000 })
  if (!done.ok) throw await errorOf(done, "Could not finish the upload")
  return fileId
}

export const RequestSignatureTool = Tool.define("request_signature", {
  description: DESCRIPTION,
  parameters,
  async execute(params, ctx) {
    if (!process.env.ALLTERNIT_API_TOKEN?.trim())
      throw new Error(
        "Not signed in for signature requests. The user must sign in to Allternit and set ALLTERNIT_API_TOKEN (an API key) before a signature can be requested.",
      )

    if (params.id) {
      const res = await platformFetch("GET", `/api/v1/sign-requests/${encodeURIComponent(params.id)}`, undefined, {
        timeoutMs: 30_000,
      })
      if (!res.ok) throw await errorOf(res, "Could not read the signature request")
      const r = (await res.json()) as SignRequest
      const metadata: Metadata = {
        id: r.id,
        status: r.status,
        sign_url: signUrl(r.id),
        title: r.title,
        signer_name: r.signer_name,
        resolved_at: r.resolved_at,
      }
      const who = r.status === "signed" ? ` by ${r.signer_name ?? "the user"}${r.resolved_at ? ` at ${r.resolved_at}` : ""}` : ""
      return {
        title: `Signature ${r.status}`,
        output: JSON.stringify({ id: r.id, status: r.status, signer_name: r.signer_name, signed_at: r.status === "signed" ? r.resolved_at : undefined }) + `\nStatus: ${r.status}${who}.`,
        metadata,
      }
    }

    if (!params.title || !params.document_path)
      throw new Error("Pass `title` and `document_path` to request a signature, or `id` to check one.")
    const file = path.resolve(params.document_path)
    if (path.extname(file).toLowerCase() !== ".pdf") throw new Error("Only PDF documents can be signed.")
    const info = await stat(file).catch(() => undefined)
    if (!info?.isFile()) throw new Error(`No file at ${file}.`)
    if (info.size > MAX_BYTES) throw new Error("The PDF is over 20 MB.")
    const bytes = await readFile(file)
    if (bytes.subarray(0, 5).toString("latin1") !== "%PDF-") throw new Error("That file is not a PDF.")

    await ctx.ask({
      permission: "request_signature",
      patterns: [file],
      always: ["*"],
      metadata: { title: params.title, document_path: file },
    })

    const sha = createHash("sha256").update(bytes).digest("hex")
    const fileId = await upload(file, bytes)
    const res = await platformFetch(
      "POST",
      "/api/v1/sign-requests",
      {
        title: params.title,
        summary: params.summary,
        details: params.details,
        document_file_id: fileId,
        document_sha256: sha,
        send_to: params.send_to,
        requester_kind: "agent",
        requester_name: ctx.agent,
        source_surface: "gizzi",
        thread_id: ctx.sessionID,
      },
      { timeoutMs: 30_000 },
    )
    if (!res.ok) throw await errorOf(res, "Could not file the signature request")
    const r = (await res.json()) as SignRequest
    const url = signUrl(r.id)
    const metadata: Metadata = { id: r.id, status: r.status, sign_url: url, title: params.title }
    return {
      title: `Signature requested: ${params.title}`,
      output: JSON.stringify({ id: r.id, status: r.status, sign_url: url }) +
        `\nAsked the user to sign. They sign on Desktop or phone; you cannot sign for them. Continue other work; check later with {id: "${r.id}"}.`,
      metadata,
    }
  },
})
