/**
 * Skill growth for the person at the keyboard: the session-start check (sync
 * with the server, then a one-line notice when approved skills wait to be
 * turned on) and the one activation path shared by the `skill_activate` tool,
 * `/knowledge` and `gizzi knowledge activate`. Every activation goes through a
 * `skill_manage` ask first; nothing here turns a skill on by itself.
 */
import { SkillGrowth } from "@/runtime/skills/growth"

export interface SkillManageRequest {
  permission: "skill_manage"
  patterns: string[]
  always: string[]
  metadata: Record<string, unknown>
}

/** The `skill_manage` ask: resolves to go ahead, throws to refuse. */
export type SkillManageAsk = (request: SkillManageRequest) => Promise<unknown>

export class SkillActivationRefused extends Error {
  constructor(message: string) {
    super(message)
    this.name = "SkillActivationRefused"
  }
}

export function waitingNotice(count: number): string {
  return count === 1
    ? "1 approved skill is waiting to be turned on: run /knowledge"
    : `${count} approved skills are waiting to be turned on: run /knowledge`
}

/**
 * Session start: sync skill growth with the server (about 2 s, then it keeps
 * going in the background) and return the notice when approved skills wait
 * for activation. Never throws; never activates. Incognito sessions skip it.
 */
export async function startupCheck(options: { incognito?: boolean; timeoutMs?: number } = {}): Promise<string | undefined> {
  if (options.incognito) return undefined
  try {
    let timer: ReturnType<typeof setTimeout> | undefined
    await Promise.race([
      SkillGrowth.sync().catch(() => {}),
      new Promise<void>((resolve) => {
        timer = setTimeout(resolve, options.timeoutMs ?? 2_000)
      }),
    ])
    if (timer) clearTimeout(timer)
    const waiting = await SkillGrowth.awaitingActivation()
    return waiting.length ? waitingNotice(waiting.length) : undefined
  } catch {
    return undefined
  }
}

/** Ask for `skill_manage` (pattern `activate:<id>`), then write the approved version to SKILL.md. */
export async function activateSkillProposal(id: string, ask: SkillManageAsk): Promise<SkillGrowth.Record> {
  await ask({ permission: "skill_manage", patterns: [`activate:${id}`], always: [], metadata: {} })
  return SkillGrowth.activate(id)
}

/**
 * The `skill_manage` ask for a person typing a command (no agent session):
 * the configured rule decides first (deny refuses, allow goes ahead), and
 * "ask" (the default) asks `confirm`. With no way to ask, it refuses.
 * Runs inside an Instance (the caller provides one).
 */
export function personAsk(confirm?: (question: string) => Promise<boolean>, question = "Turn on this skill?"): SkillManageAsk {
  return async (request) => {
    const action = await configuredAction(request.patterns[0] ?? "activate")
    if (action === "deny") throw new SkillActivationRefused("Turning on skills is blocked by your skill_manage permission setting.")
    if (action === "allow") return
    if (!confirm) throw new SkillActivationRefused("Not turned on: there was no one to ask (in a script, pass --yes).")
    if (!(await confirm(question))) throw new SkillActivationRefused("Not turned on.")
  }
}

async function configuredAction(pattern: string): Promise<"allow" | "deny" | "ask"> {
  try {
    const [{ Agent }, { PermissionNext }] = await Promise.all([
      import("@/runtime/loop/agent"),
      import("@/runtime/tools/guard/permission/next"),
    ])
    const agent = await Agent.get("build")
    return PermissionNext.evaluate("skill_manage", pattern, agent?.permission ?? []).action
  } catch {
    return "ask"
  }
}
