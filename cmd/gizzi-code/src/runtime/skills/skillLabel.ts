// Row label for the app proxy tool `load_skill` (mcp.allternit.com): "Loaded skill · <name> · from <from>".
const LOAD_SKILL = /(^|_)load_skill$/

export function isLoadSkillTool(toolName: string): boolean {
  return LOAD_SKILL.test(toolName)
}

/** Line 1 of the result: `Skill <id> from <from>. This text comes from that server.` */
export function parseSkillHeader(text: string): { id: string; from: string } | undefined {
  const m = /^Skill (\S+) from (.+?)\. /.exec(text.split("\n", 1)[0] ?? "")
  return m ? { id: m[1]!, from: m[2]! } : undefined
}

export function loadSkillLabel(
  toolName: string,
  input?: { id?: unknown },
  result?: { text?: string; structuredContent?: Record<string, unknown> },
): string | undefined {
  if (!isLoadSkillTool(toolName)) return undefined
  const sc = result?.structuredContent?.allternitSkill as { name?: string; id?: string; from?: string } | undefined
  const head = result?.text ? parseSkillHeader(result.text) : undefined
  const name = sc?.name ?? sc?.id ?? head?.id ?? (typeof input?.id === "string" ? input.id : undefined)
  const from = sc?.from ?? head?.from
  if (!name) return "Loaded skill"
  return from ? `Loaded skill · ${name} · from ${from}` : `Loaded skill · ${name}`
}
