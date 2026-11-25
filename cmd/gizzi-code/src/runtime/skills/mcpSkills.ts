// MCP skills: a server that advertises resources may publish `skill://<name>/SKILL.md`
// resources (SEP-2640-style). Each becomes a prompt command `mcp__<server>__skill_<name>`
// whose body is the resource text, so it behaves like any other skill in the session.
// Gated by the MCP_SKILLS feature flag (script/features.mjs); on by default since 2026-11-24.
const cache = new Map<string, unknown>()

const SKILL_SCHEME = "skill://"

function safe(name: string): string {
  return name.replace(/[^a-zA-Z0-9_-]/g, "_")
}

/** `skill://notes/SKILL.md` -> "notes"; `skill://notes` -> "notes". */
export function skillNameFromUri(uri: string): string | undefined {
  if (!uri.startsWith(SKILL_SCHEME)) return undefined
  const parts = uri.slice(SKILL_SCHEME.length).split("/").filter(Boolean)
  if (parts.length === 0) return undefined
  const last = parts[parts.length - 1]!
  const name = parts.length > 1 && /^skill\.md$/i.test(last) ? parts[parts.length - 2]! : last.replace(/\.md$/i, "")
  return name ? safe(decodeURIComponent(name)) : undefined
}

async function fetchMcpSkills(connection: any): Promise<unknown[]> {
  if (!connection || connection.type !== "connected") return []
  try {
    const listed = (await connection.client.request({ method: "resources/list" })) as {
      resources?: { uri: string; name?: string; description?: string }[]
    }
    const server = safe(String(connection.name))
    const seen = new Set<string>()
    const commands: unknown[] = []
    for (const resource of listed.resources ?? []) {
      const skill = skillNameFromUri(resource.uri)
      if (!skill || seen.has(skill)) continue
      seen.add(skill)
      const description = resource.description ?? resource.name ?? ""
      commands.push({
        type: "prompt" as const,
        name: `mcp__${server}__skill_${skill}`,
        description,
        hasUserSpecifiedDescription: !!resource.description,
        contentLength: 0,
        isEnabled: () => true,
        isHidden: false,
        isMcp: true,
        progressMessage: "loading skill",
        userFacingName: () => `${connection.name}:${skill} (MCP skill)`,
        argNames: [] as string[],
        source: "mcp" as const,
        loadedFrom: "mcp" as const,
        async getPromptForCommand(args: string) {
          const read = (await connection.client.request({
            method: "resources/read",
            params: { uri: resource.uri },
          })) as { contents?: { text?: string }[] }
          const text = (read.contents ?? []).map((c) => c.text ?? "").join("\n")
          return [{ type: "text" as const, text: args ? `${text}\n\nARGUMENTS: ${args}` : text }]
        },
      })
    }
    return commands
  } catch {
    // A server without resources/list, or one that errors, just has no skills.
    return []
  }
}

export const fetchMcpSkillsForClient: {
  (client: unknown): Promise<unknown[]>
  cache: Map<string, unknown>
} = Object.assign(fetchMcpSkills, { cache })
