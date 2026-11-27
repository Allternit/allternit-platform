/**
 * Knowledge factory client + text formatting for `gizzi knowledge` and the
 * /knowledge command. Routes: CONTRACT.md "Knowledge factory". Every call is
 * scoped to the signed-in user by the server.
 */
import { createTwoFilesPatch } from "diff"
import { PlatformApiError, PlatformSignedOutError, platformRequest } from "@/runtime/bots/platform-api"
import { Instance } from "@/runtime/context/project/instance"
import { SkillGrowth } from "@/runtime/skills/growth"
import { activateSkillProposal, personAsk, SkillActivationRefused } from "@/runtime/skills/growth-person"

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

export function formatList(flags: KnowledgeFlag[], proposals: KnowledgeProposal[], waiting: SkillGrowth.Record[] = []): string {
  if (!flags.length && !proposals.length && !waiting.length) return "Nothing waiting. No open flags and no proposals to review."
  const out: string[] = []
  if (waiting.length) {
    out.push(`Approved skills waiting to be turned on (${waiting.length})`)
    for (const w of waiting) {
      out.push(`  ${w.server_id ?? w.id}  skill/${w.name} v${w.version}  ${clip(w.description, 60)}  [${w.origin === "curator" ? "curator" : "skill_growth"}]`)
    }
    if (proposals.length || flags.length) out.push("")
  }
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
  out.push("", "/knowledge show <id> · approve <id> · reject <id> · dismiss <id> · activate <id>")
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

export interface KnowledgeOptions {
  /**
   * The skill_manage ask for turning on an approved skill (a dialog in the TUI,
   * a prompt in a terminal). Without it, approving a skill only says how to
   * turn it on, and `activate` refuses unless the permission rule is "allow".
   */
  confirm?: (question: string) => Promise<boolean>
  /** Project folder for project-scope skills (default: the current folder). */
  directory?: string
}

/** Approved skills on this device that wait for activation (after a quiet sync with the server). */
async function waitingSkills(): Promise<SkillGrowth.Record[]> {
  await SkillGrowth.sync().catch(() => {})
  return SkillGrowth.awaitingActivation().catch(() => [])
}

/**
 * Turn on an approved skill proposal (local id or `kprop_…`): pull approvals,
 * ask for skill_manage, then write SKILL.md and tell the server. The only
 * activation path besides the skill_activate tool, and it asks the same way.
 */
export async function activateSkill(id: string, options: KnowledgeOptions = {}): Promise<string> {
  return Instance.provide({
    directory: options.directory ?? process.cwd(),
    fn: async () => {
      await SkillGrowth.sync()
      const record = await SkillGrowth.find(id)
      if (!record || record.status !== "approved") {
        const waiting = await SkillGrowth.awaitingActivation()
        const state = record ? `${id} is ${record.status}, not approved.` : `No approved skill proposal ${id} on this device.`
        return waiting.length
          ? `${state} Waiting to be turned on: ${waiting.map((w) => `${w.server_id ?? w.id} (${w.name})`).join(", ")}.`
          : state
      }
      const where = record.scope === "project" ? "this project" : "your skills"
      const who = record.origin === "curator" ? "curator edit" : "skill growth"
      const question = `Turn on skill "${record.name}" v${record.version} (${who}) in ${where}?`
      try {
        const done = await activateSkillProposal(id, personAsk(options.confirm, question))
        return `Turned on ${done.name} v${done.version}: ${done.target}`
      } catch (error) {
        if (error instanceof SkillActivationRefused) return `${error.message} Turn it on later with /knowledge activate ${id}.`
        throw error
      }
    },
  })
}

/** Run one subcommand and return the text to show. Shared by /knowledge and `gizzi knowledge`. */
export async function runKnowledge(args: string[], options: KnowledgeOptions = {}): Promise<string> {
  const [sub = "list", id] = args
  try {
    if (sub === "list") return formatList(await KnowledgeApi.flags(), await KnowledgeApi.proposals(), await waitingSkills())
    if (sub === "activate") {
      if (!id) return "Pass a proposal id: /knowledge activate <id>"
      return await activateSkill(id, options)
    }
    if (sub === "show") {
      if (!id) return "Pass a proposal id: /knowledge show <id>"
      const p = (await KnowledgeApi.proposals()).find((x) => x.id === id)
      return p ? formatProposal(p) : `No proposal ${id} waiting for review.`
    }
    if (sub === "approve" || sub === "reject" || sub === "dismiss") {
      if (!id) return `Pass an id: /knowledge ${sub} <id>`
      const r = await KnowledgeApi.act(sub, id)
      if (sub === "approve" && "target_kind" in r && (r as KnowledgeProposal).target_kind === "skill") {
        if (!options.confirm) return `${id} approved. Turn it on with /knowledge activate ${id} (it asks first).`
        return `${id} approved. ${await activateSkill(id, options)}`
      }
      return `${id} ${sub === "approve" ? "approved" : sub === "reject" ? "rejected" : "dismissed"}.`
    }
    return "Usage: /knowledge [list] | show <id> | approve <id> | reject <id> | dismiss <id> | activate <id>"
  } catch (error) {
    return describeError(error)
  }
}
