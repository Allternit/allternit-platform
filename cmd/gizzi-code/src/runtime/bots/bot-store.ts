/**
 * Bot profile store (Bot Mode, phase B1 — see docs/programs/gizzi/GIZZI_BOT_MODE_SPEC.md).
 *
 * A Bot is a profile, not a new runtime primitive (D1): identity metadata in
 * `bot.json`, persona/standing instructions in `SOUL.md`, and bot-scoped
 * notes in `memory/`. Bots live at `~/.gizzi/bots/<name>/` (override the
 * config home with `GIZZI_CONFIG_DIR`, the same override pattern used by
 * plugin state and auto-memory).
 *
 * This module is deliberately free of CLI/UI/settings imports so it can be
 * driven directly from bun tests. Phase B2 builds canonical-session chat on
 * top of `canonicalSession`; phase B3 adds routines.
 */
import { existsSync } from "node:fs"
import { cp, mkdir, readdir, readFile, rename, rm, stat, writeFile } from "node:fs/promises"
import { homedir } from "node:os"
import { join, normalize } from "node:path"
import z from "zod/v4"

export class BotStoreError extends Error {}

/* -------------------------------------------------------------------------- */
/* Config home (~/.gizzi) — same resolution as pluginDirectories.ts /         */
/* memdir/paths.ts: GIZZI_CONFIG_DIR wins, then ~/.gizzi. GIZZI_TEST_HOME is  */
/* honored like GlobalPaths.home so test harnesses can sandbox $HOME.         */
/* -------------------------------------------------------------------------- */

export function gizziConfigHome(): string {
  return (
    process.env.GIZZI_CONFIG_DIR ??
    join(process.env.GIZZI_TEST_HOME || homedir(), ".gizzi")
  ).normalize("NFC")
}

/** Parent directory holding every bot: `<config home>/bots`. */
export function botsDir(): string {
  return join(gizziConfigHome(), "bots")
}

/** Home directory of one bot: `<config home>/bots/<name>`. */
export function botDir(name: string): string {
  return join(botsDir(), name)
}

/* -------------------------------------------------------------------------- */
/* Schema (D1)                                                                */
/* -------------------------------------------------------------------------- */

export const BotAvatarSchema = z.object({
  kind: z.literal("generated"),
})

export const BotCanonicalSessionSchema = z.object({
  projectPath: z.string(),
  sessionId: z.string(),
})

/**
 * The platform agent this bot is registered as (so Desktop, the pet HUD and
 * the terminal `/bots` chat share one conversation). `syncHash` is the hash
 * of the identity last pushed; a different hash means push again.
 */
export const BotPlatformLinkSchema = z.object({
  id: z.string(),
  syncHash: z.string().nullable(),
})

export const BotSchema = z.object({
  schemaVersion: z.literal(1),
  name: z.string(),
  title: z.string(),
  description: z.string(),
  model: z.string().nullable(),
  avatar: BotAvatarSchema.nullable(),
  canonicalSession: BotCanonicalSessionSchema.nullable(),
  capabilityEpoch: z.string().nullable(),
  /** Absent in bot.json written before registration existed. */
  platform: BotPlatformLinkSchema.nullable().optional(),
  createdAt: z.string(),
  updatedAt: z.string(),
})

export type BotAvatar = z.infer<typeof BotAvatarSchema>
export type BotCanonicalSession = z.infer<typeof BotCanonicalSessionSchema>
export type BotPlatformLink = z.infer<typeof BotPlatformLinkSchema>
export type Bot = z.infer<typeof BotSchema>

/** Fields `updateBot` accepts — identity metadata only, never the canonical session. */
export interface BotPatch {
  title?: string
  description?: string
  /** Pass null to clear the model pin. */
  model?: string | null
}

/* -------------------------------------------------------------------------- */
/* Name rules                                                                 */
/* -------------------------------------------------------------------------- */

const SLUG_RE = /^[a-z0-9]+(-[a-z0-9]+)*$/

/**
 * Subcommand names of `gizzi agents bot` (plus a few generic collisions). A bot
 * named `list` or `chat` would be unreachable from the CLI, so creation is
 * refused up front.
 */
export const RESERVED_BOT_NAMES = new Set([
  "create",
  "list",
  "show",
  "edit",
  "clone",
  "delete",
  "chat",
  "routine",
  "help",
  "new",
  "config",
  "default",
])

export function validateBotName(name: string): void {
  if (!name) throw new BotStoreError("bot name is required")
  if (!SLUG_RE.test(name)) {
    throw new BotStoreError(
      `invalid bot name '${name}' — names must be a lowercase slug of letters, digits, and dashes (e.g. 'research-buddy')`,
    )
  }
  if (RESERVED_BOT_NAMES.has(name)) {
    throw new BotStoreError(`'${name}' is a reserved name — it collides with a gizzi agents bot subcommand`)
  }
}

/**
 * Resolve a user-supplied name against a directory listing. An exact match
 * wins; otherwise a case-insensitive unique match resolves; zero matches
 * return null; more than one match (possible only on case-sensitive
 * filesystems) is an error rather than a guess.
 */
export function resolveBotDirName(entries: string[], name: string): string | null {
  if (entries.includes(name)) return name
  const lower = name.toLowerCase()
  const matches = entries.filter((e) => e.toLowerCase() === lower)
  if (matches.length === 0) return null
  if (matches.length > 1) {
    throw new BotStoreError(
      `bot name '${name}' is ambiguous — it matches: ${matches.join(", ")}`,
    )
  }
  return matches[0]
}

/* -------------------------------------------------------------------------- */
/* Filesystem helpers                                                         */
/* -------------------------------------------------------------------------- */

async function botJsonPath(dir: string): Promise<string> {
  return join(dir, "bot.json")
}

/** Write `data` as JSON atomically: tmp file in the same directory, then rename. */
async function writeJsonAtomic(filePath: string, data: unknown): Promise<void> {
  const tmp = `${filePath}.${process.pid}.${Math.random().toString(36).slice(2)}.tmp`
  try {
    await writeFile(tmp, JSON.stringify(data, null, 2) + "\n", { mode: 0o600 })
    await rename(tmp, filePath)
  } catch (e) {
    await rm(tmp, { force: true }).catch(() => {})
    throw e
  }
}

async function readBot(dir: string): Promise<Bot> {
  const name = dir.split("/").pop() ?? dir
  const raw = await readFile(await botJsonPath(dir), "utf8")
  let json: unknown
  try {
    json = JSON.parse(raw)
  } catch {
    throw new BotStoreError(`bot '${name}' has a malformed bot.json (not valid JSON)`)
  }
  const parsed = BotSchema.safeParse(json)
  if (!parsed.success) {
    throw new BotStoreError(`bot '${name}' has an invalid bot.json: ${parsed.error.message}`)
  }
  return parsed.data
}

async function listBotDirNames(): Promise<string[]> {
  const root = botsDir()
  if (!existsSync(root)) return []
  const entries = await readdir(root, { withFileTypes: true })
  return entries
    .filter((e) => e.isDirectory() && existsSync(join(root, e.name, "bot.json")))
    .map((e) => e.name)
}

function defaultSoul(name: string): string {
  return `# SOUL — ${name}

Who this bot is. Fill in the persona and standing instructions below; the
content is injected into the bot's canonical chat sessions (Bot Mode, D1).

## Persona

<!-- Voice, temperament, role. Short concrete sentences beat adjectives. -->

## Standing instructions

<!-- What this bot always does, never does, and how it reports back. -->

1.
`
}

/* -------------------------------------------------------------------------- */
/* Public API                                                                 */
/* -------------------------------------------------------------------------- */

export interface CreateBotInput {
  name: string
  title: string
  description?: string
  model?: string | null
  /** Content for SOUL.md; a starter template is written when omitted. */
  soul?: string
}

export interface BotStoreOptions {
  /** Injectable clock for tests. */
  now?: Date
}

/**
 * Create a bot at `<config home>/bots/<name>/` with `bot.json`, `SOUL.md`,
 * and an empty `memory/` directory. Refuses invalid, reserved, or
 * already-taken names. Directories are created 0o700.
 */
export async function createBot(
  input: CreateBotInput,
  opts: BotStoreOptions = {},
): Promise<Bot> {
  validateBotName(input.name)
  const dir = botDir(input.name)
  if (existsSync(dir)) {
    throw new BotStoreError(`bot '${input.name}' already exists at ${dir}`)
  }

  const now = (opts.now ?? new Date()).toISOString()
  const bot: Bot = {
    schemaVersion: 1,
    name: input.name,
    title: input.title,
    description: input.description ?? "",
    model: input.model ?? null,
    avatar: null,
    canonicalSession: null,
    capabilityEpoch: null,
    createdAt: now,
    updatedAt: now,
  }

  await mkdir(join(dir, "memory"), { recursive: true, mode: 0o700 })
  await writeFile(join(dir, "SOUL.md"), input.soul ?? defaultSoul(input.name), {
    encoding: "utf8",
    mode: 0o600,
  })
  await writeJsonAtomic(await botJsonPath(dir), bot)
  return bot
}

/** List all bots with a valid `bot.json`, sorted by name. Broken profiles are skipped. */
export async function listBots(): Promise<Bot[]> {
  const root = botsDir()
  const names = await listBotDirNames()
  const bots: Bot[] = []
  for (const name of names) {
    try {
      bots.push(await readBot(join(root, name)))
    } catch {
      // unreadable/invalid profile — skip it rather than failing the listing
    }
  }
  return bots.sort((a, b) => a.name.localeCompare(b.name))
}

/**
 * Fetch one bot. Lookup is case-insensitive; an exact directory match wins,
 * a unique case-insensitive match resolves, multiple matches throw. Returns
 * null when no bot exists under that name.
 */
export async function getBot(name: string): Promise<Bot | null> {
  const entries = await listBotDirNames()
  const resolved = resolveBotDirName(entries, name)
  if (resolved === null) return null
  return readBot(botDir(resolved))
}

/**
 * Patch identity metadata (title / description / model) and bump `updatedAt`.
 * Writes through atomically. The canonical session pointer is never touched
 * here — B2 owns it.
 */
export async function updateBot(
  name: string,
  patch: BotPatch,
  opts: BotStoreOptions = {},
): Promise<Bot> {
  const entries = await listBotDirNames()
  const resolved = resolveBotDirName(entries, name)
  if (resolved === null) {
    throw new BotStoreError(`bot '${name}' not found`)
  }
  const dir = botDir(resolved)
  const bot = await readBot(dir)
  if (patch.title !== undefined) bot.title = patch.title
  if (patch.description !== undefined) bot.description = patch.description
  if (patch.model !== undefined) bot.model = patch.model
  bot.updatedAt = (opts.now ?? new Date()).toISOString()
  await writeJsonAtomic(await botJsonPath(dir), bot)
  return bot
}

/**
 * Pin (or re-point, or clear) the bot's canonical session pointer (D2). B2
 * owns the only write path for `canonicalSession`; identity edits via
 * `updateBot` never touch it.
 */
export async function pinCanonicalSession(
  name: string,
  canonicalSession: BotCanonicalSession | null,
  opts: BotStoreOptions = {},
): Promise<Bot> {
  const entries = await listBotDirNames()
  const resolved = resolveBotDirName(entries, name)
  if (resolved === null) {
    throw new BotStoreError(`bot '${name}' not found`)
  }
  const dir = botDir(resolved)
  const bot = await readBot(dir)
  bot.canonicalSession = canonicalSession
  bot.updatedAt = (opts.now ?? new Date()).toISOString()
  await writeJsonAtomic(await botJsonPath(dir), bot)
  return bot
}

/**
 * Stamp the bot's capability epoch (D5). Written when the persona injection
 * is (re)built so the next session open can detect drift cheaply.
 */
export async function setCapabilityEpoch(
  name: string,
  capabilityEpoch: string | null,
  opts: BotStoreOptions = {},
): Promise<Bot> {
  const entries = await listBotDirNames()
  const resolved = resolveBotDirName(entries, name)
  if (resolved === null) {
    throw new BotStoreError(`bot '${name}' not found`)
  }
  const dir = botDir(resolved)
  const bot = await readBot(dir)
  bot.capabilityEpoch = capabilityEpoch
  bot.updatedAt = (opts.now ?? new Date()).toISOString()
  await writeJsonAtomic(await botJsonPath(dir), bot)
  return bot
}

/** Record (or clear) the platform agent this bot is registered as. */
export async function setPlatformLink(
  name: string,
  platform: BotPlatformLink | null,
  opts: BotStoreOptions = {},
): Promise<Bot> {
  const entries = await listBotDirNames()
  const resolved = resolveBotDirName(entries, name)
  if (resolved === null) {
    throw new BotStoreError(`bot '${name}' not found`)
  }
  const dir = botDir(resolved)
  const bot = await readBot(dir)
  bot.platform = platform
  bot.updatedAt = (opts.now ?? new Date()).toISOString()
  await writeJsonAtomic(await botJsonPath(dir), bot)
  return bot
}

/**
 * Clone a bot: copies identity fields, SOUL.md, and every memory note.
 * `canonicalSession` is never copied (a clone starts un-pinned; B2 resolves
 * its own chat) and `capabilityEpoch` is reset so the injection rebuilds.
 */
export async function cloneBot(
  sourceName: string,
  newName: string,
  opts: BotStoreOptions = {},
): Promise<Bot> {
  const source = await getBot(sourceName)
  if (!source) {
    throw new BotStoreError(`bot '${sourceName}' not found`)
  }
  const soul = await readSoul(source.name)
  const clone = await createBot(
    {
      name: newName,
      title: source.title,
      description: source.description,
      model: source.model,
      soul: soul ?? undefined,
    },
    opts,
  )

  const sourceMemory = join(botDir(source.name), "memory")
  const cloneMemory = join(botDir(clone.name), "memory")
  const notes = existsSync(sourceMemory)
    ? (await readdir(sourceMemory, { withFileTypes: true }))
        .filter((e) => e.isFile())
        .map((e) => e.name)
    : []
  for (const note of notes) {
    await cp(join(sourceMemory, note), join(cloneMemory, note))
  }
  return clone
}

/** Delete a bot's entire home directory. Throws when the bot does not exist. */
export async function deleteBot(name: string): Promise<Bot> {
  const entries = await listBotDirNames()
  const resolved = resolveBotDirName(entries, name)
  if (resolved === null) {
    throw new BotStoreError(`bot '${name}' not found`)
  }
  const bot = await readBot(botDir(resolved))
  await rm(botDir(resolved), { recursive: true, force: true })
  return bot
}

/** Read the bot's SOUL.md persona file. Returns null when it does not exist. */
export async function readSoul(name: string): Promise<string | null> {
  const entries = await listBotDirNames()
  const resolved = resolveBotDirName(entries, name)
  if (resolved === null) {
    throw new BotStoreError(`bot '${name}' not found`)
  }
  const path = join(botDir(resolved), "SOUL.md")
  if (!existsSync(path)) return null
  return readFile(path, "utf8")
}

/** List note file names in the bot's `memory/` directory (sorted). */
export async function listBotMemory(name: string): Promise<string[]> {
  const entries = await listBotDirNames()
  const resolved = resolveBotDirName(entries, name)
  if (resolved === null) {
    throw new BotStoreError(`bot '${name}' not found`)
  }
  const memory = join(botDir(resolved), "memory")
  if (!existsSync(memory)) return []
  const files = await readdir(memory, { withFileTypes: true })
  const names: string[] = []
  for (const f of files) {
    if (f.isFile()) names.push(f.name)
  }
  return names.sort()
}

/**
 * Best-effort directory-permission check used by tests: the bot directory
 * must give no permissions to group/other.
 */
export async function botDirIsPrivate(name: string): Promise<boolean> {
  const info = await stat(botDir(name))
  return (info.mode & 0o077) === 0
}
