/**
 * Knowledge factory client + text formatting for `gizzi knowledge` and the
 * /knowledge command. Routes: CONTRACT.md "Knowledge factory". Every call is
 * scoped to the signed-in user by the server.
 */
import { createTwoFilesPatch } from "diff"
import { PlatformApiError, PlatformSignedOutError, platformRequest } from "@/runtime/bots/platform-api"

export interface KnowledgeFlag {
  id: string
  kind: string
  target_kind: string
  target_ref?: string | null
  evidence: string
  suggested_fix?: string | null
  status: string
  created_at?: string
}

export interface KnowledgeProposal {
  id: string
  target_kind: string
  target_ref: string
  title: string
  body: string
  base?: string | null
  evidence?: Array<{ flag_id?: string; observation_id?: string }>
  origin: string
  status: string
  created_at?: string
}

export type Action = "approve" | "reject" | "dismiss"

export const KnowledgeApi = {
  async flags(): Promise<KnowledgeFlag[]> {
    return (await platformRequest<{ flags: KnowledgeFlag[] }>("GET", "/api/v1/knowledge/flags?status=open")).flags ?? []
  },
  async proposals(): Promise<KnowledgeProposal[]> {
    return (await platformRequest<{ proposals: KnowledgeProposal[] }>("GET", "/api/v1/knowledge/proposals?status=proposed")).proposals ?? []
  },
  async act(action: Action, id: string): Promise<KnowledgeProposal | KnowledgeFlag> {
    const path =
      action === "dismiss" ? `/api/v1/knowledge/flags/${encodeURIComponent(id)}/dismiss` : `/api/v1/knowledge/proposals/${encodeURIComponent(id)}/${action}`
    const r = await platformRequest<{ flag?: KnowledgeFlag; proposal?: KnowledgeProposal }>("POST", path, {})
    return (r.proposal ?? r.flag)!
  },
}

const clip = (s: string, n: number) => {
  const one = s.replace(/\s+/g, " ").trim()
  return one.length > n ? `${one.slice(0, n - 1)}…` : one
}

export function formatList(flags: KnowledgeFlag[], proposals: KnowledgeProposal[]): string {
  if (!flags.length && !proposals.length) return "Nothing waiting. No open flags and no proposals to review."
  const out: string[] = []
  if (proposals.length) {
    out.push(`Proposals to review (${proposals.length})`)
    for (const p of proposals) {
      out.push(`  ${p.id}  ${p.target_kind}/${p.target_ref}  ${clip(p.title, 70)}  [${p.origin}]`)
    }
  }
  if (flags.length) {
    if (out.length) out.push("")
    out.push(`Open flags (${flags.length})`)
    for (const f of flags) {
      out.push(`  ${f.id}  ${f.kind} ${f.target_kind}${f.target_ref ? `/${f.target_ref}` : ""}  ${clip(f.evidence, 80)}`)
    }
  }
  out.push("", "/knowledge show <id> · approve <id> · reject <id> · dismiss <id>")
  return out.join("\n")
}

/** A proposal's body as a diff against `base` (or the plain body when new), plus its evidence. */
export function formatProposal(p: KnowledgeProposal): string {
  const head = `${p.id}  ${p.target_kind}/${p.target_ref}  [${p.origin}]\n${p.title}`
  const body = p.base
    ? createTwoFilesPatch(`${p.target_ref} (current)`, `${p.target_ref} (proposed)`, p.base, p.body, "", "", { context: 3 })
    : p.body
  const ev = (p.evidence ?? []).map((e) => `  ${e.flag_id ? `flag ${e.flag_id}` : `observation ${e.observation_id}`}`)
  return [head, "", body.trimEnd(), "", ev.length ? `Evidence:\n${ev.join("\n")}` : "Evidence: none listed"].join("\n")
}

export function describeError(error: unknown): string {
  if (error instanceof PlatformSignedOutError) return "Not signed in to Allternit. Run `gizzi login` first."
  if (error instanceof PlatformApiError) return error.message
  const text = error instanceof Error ? error.message : String(error)
  return /ECONNREFUSED|ConnectionRefused|Unable to connect|fetch failed|ENOTFOUND|timed out/i.test(text)
    ? "Cannot reach the Allternit runtime. Start Allternit Desktop (or the local allternit-api) and try again."
    : text
}

/** Run one subcommand and return the text to show. Shared by /knowledge and `gizzi knowledge`. */
export async function runKnowledge(args: string[]): Promise<string> {
  const [sub = "list", id] = args
  try {
    if (sub === "list") return formatList(await KnowledgeApi.flags(), await KnowledgeApi.proposals())
    if (sub === "show") {
      if (!id) return "Pass a proposal id: /knowledge show <id>"
      const p = (await KnowledgeApi.proposals()).find((x) => x.id === id)
      return p ? formatProposal(p) : `No proposal ${id} waiting for review.`
    }
    if (sub === "approve" || sub === "reject" || sub === "dismiss") {
      if (!id) return `Pass an id: /knowledge ${sub} <id>`
      const r = await KnowledgeApi.act(sub, id)
      if (sub === "approve" && "target_kind" in r && (r as KnowledgeProposal).target_kind === "skill") {
        return `${id} approved: activation runs in step 13`
      }
      return `${id} ${sub === "approve" ? "approved" : sub === "reject" ? "rejected" : "dismissed"}.`
    }
    return "Usage: /knowledge [list] | show <id> | approve <id> | reject <id> | dismiss <id>"
  } catch (error) {
    return describeError(error)
  }
}
