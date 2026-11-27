// MCP skills: a server that advertises resources may publish `skill://<name>/SKILL.md`
// resources (SEP-2640-style). Each becomes a prompt command `mcp__<server>__skill_<name>`
// whose body is the resource text, so it behaves like any other skill in the session.
// Gated by the MCP_SKILLS feature flag (script/features.mjs); on by default since 2026-11-24.
const cache = new Map<string, unknown>()

const SKILL_SCHEME = "skill://"

function safe(name: string): string {
  return name.replace(/[^a-zA-Z0-9_-]/g, "_")
}

export type SkillSource = "server" | "app" | "user" | "org"

export type SkillRef = { name: string; source: SkillSource; from?: string }

function seg(v: string): string {
  try {
    return safe(decodeURIComponent(v))
  } catch {
    return safe(v)
  }
}

/**
 * Parse a skill URI into name, source and origin.
 * `skill://notes/SKILL.md` -> notes (server); `skill://user/x/SKILL.md` -> x (user);
 * `skill://org/<org>/x/SKILL.md` -> x (org); `skill://app/<slug>/x/SKILL.md` -> x (app, from <slug>).
 */
export function parseSkillUri(uri: string): SkillRef | undefined {
  if (!uri.startsWith(SKILL_SCHEME)) return undefined
  let parts = uri.slice(SKILL_SCHEME.length).split("/").filter(Boolean)
  if (parts.length === 0) return undefined
  let source: SkillSource = "server"
  let from: string | undefined
  if (parts.length > 2 && parts[0] === "app") {
    source = "app"
    from = seg(parts[1]!)
    parts = parts.slice(2)
  } else if (parts.length > 2 && parts[0] === "org") {
    source = "org"
    from = seg(parts[1]!)
    parts = parts.slice(2)
  } else if (parts.length > 1 && parts[0] === "user") {
    source = "user"
    parts = parts.slice(1)
  }
  const last = parts[parts.length - 1]!
  const name = parts.length > 1 && /^skill\.md$/i.test(last) ? parts[parts.length - 2]! : last.replace(/\.md$/i, "")
  return name ? { name: seg(name), source, from } : undefined
}

/** `skill://notes/SKILL.md` -> "notes"; `skill://app/tips/tips/SKILL.md` -> "tips". */
export function skillNameFromUri(uri: string): string | undefined {
  return parseSkillUri(uri)?.name
}

/** Command-name stem; a collision between sources gets a source prefix (`app-tips:tips`). */
export function uniqueSkillKey(ref: SkillRef, taken: Set<string>): string | undefined {
  if (!taken.has(ref.name)) return ref.name
  const prefix = ref.source === "app" ? `app-${ref.from}` : ref.source === "org" ? `org-${ref.from}` : ref.source
  const key = `${prefix}:${ref.name}`
  return taken.has(key) ? undefined : key
}

/** User-facing origin text. */
export function skillOriginLabel(ref: SkillRef, server: string): string {
  if (ref.source === "app") return `from the ${ref.from} app`
  if (ref.source === "user") return "your skill"
  if (ref.source === "org") return "org skill"
  return `${server} skill`
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
      const ref = parseSkillUri(resource.uri)
      if (!ref) continue
      const skill = uniqueSkillKey(ref, seen)
      if (!skill) continue
      seen.add(skill)
      const description = resource.description ?? resource.name ?? ""
      commands.push({
        type: "prompt" as const,
        name: `mcp__${server}__skill_${safe(skill)}`,
        description,
        hasUserSpecifiedDescription: !!resource.description,
        contentLength: 0,
        isEnabled: () => true,
        isHidden: false,
        isMcp: true,
        progressMessage: "loading skill",
        userFacingName: () => `${connection.name}:${skill} (${skillOriginLabel(ref, String(connection.name))})`,
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
