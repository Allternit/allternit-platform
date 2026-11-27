/**
 * `gizzi agents bot` — Bot Mode command group (phases B1+B2+B3 — see
 * docs/programs/gizzi/GIZZI_BOT_MODE_SPEC.md).
 *
 * A Bot is a profile (D1): `~/.gizzi/bots/<name>/` holds `bot.json`
 * (identity), `SOUL.md` (persona), and `memory/` (bot-scoped notes). This
 * group manages those profiles, opens canonical chats (D2/D6), and manages
 * routines (B3/D3): cron jobs of type `agent` with `config.bot` set that
 * deliver their prompt into the bot's canonical chat, namespaced
 * `[bot:<name>] <label>`.
 */
import { readFile } from "node:fs/promises"
import { existsSync } from "node:fs"
import { cmd } from "@/cli/commands/cmd"
import { UI } from "@/cli/ui"
import * as prompts from "@clack/prompts"
import { bootstrap } from "@/cli/bootstrap"
import { RunCommand } from "@/cli/commands/run"
import { BotThreadsCommand } from "@/cli/commands/thread"
import { TuiThreadCommand } from "@/cli/ui/ink-app/thread"
import { describeSchedule } from "@/runtime/automation/cron/parser"
import {
  BotStoreError,
  botDir,
  cloneBot,
  createBot,
  deleteBot,
  getBot,
  listBotMemory,
  listBots,
  readSoul,
  updateBot,
  type Bot,
  type BotBrain,
} from "@/runtime/bots/bot-store"
import { platformRequest } from "@/runtime/bots/platform-api"
import {
  allowed,
  fits,
  getRuntime,
  listRuntimes,
  planRefusal,
  resolveRuntime,
  type AccessKind,
  type BotBrainLike,
  type RuntimeEntry,
} from "@/runtime/providers/discovery/runtime-registry"
import { cliCatalog } from "@/runtime/providers/discovery/subprocess"
import {
  ensureCapabilityEpoch,
  openCanonicalChat,
} from "@/runtime/bots/canonical-chat"
import {
  addBotRoutine,
  botRoutineLabel,
  cronServiceDeps,
  listBotRoutines,
  removeBotRoutine,
} from "@/runtime/bots/bot-routines"
import { classifyFailure } from "@/runtime/bots/failure-reasons"
import { Bus } from "@/shared/bus"
import { Session } from "@/runtime/session"

async function guard(fn: () => Promise<void>): Promise<void> {
  try {
    await fn()
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e)
    UI.println(UI.Style.TEXT_ERROR + `❌ ${message}` + UI.Style.RESET)
    if (process.env.GIZZI_BOT_DEBUG) console.error(e)
    process.exit(1)
  }
}

function exitOk(): never {
  process.exit(0)
}

/* -------------------------------------------------------------------------- */
/* runtime + brain (Harness & Bots 2.0: --runtime / --brain)                  */
/* -------------------------------------------------------------------------- */

const PROVIDER_NAMES: Record<string, string> = {
  anthropic: "Anthropic",
  openai: "OpenAI",
  google: "Google",
  xai: "xAI",
  moonshot: "Moonshot",
}

function providerName(id: string): string {
  return PROVIDER_NAMES[id] ?? id.charAt(0).toUpperCase() + id.slice(1)
}

function editDistance(a: string, b: string): number {
  const row = Array.from({ length: b.length + 1 }, (_, j) => j)
  for (let i = 1; i <= a.length; i++) {
    let prev = row[0]!
    row[0] = i
    for (let j = 1; j <= b.length; j++) {
      const keep = row[j]!
      row[j] = Math.min(row[j]! + 1, row[j - 1]! + 1, prev + (a[i - 1] === b[j - 1] ? 0 : 1))
      prev = keep
    }
  }
  return row[b.length]!
}

/** Runtime ids whose id or alias is close to `input` (substring or a small typo). */
export function closeRuntimeIds(input: string): string[] {
  const key = input.toLowerCase()
  const scored: { id: string; score: number }[] = []
  for (const r of listRuntimes()) {
    const names = [r.id, r.name, ...(r.aliases ?? [])].map((n) => n.toLowerCase())
    let best = Infinity
    for (const n of names) {
      if (n.includes(key) || key.includes(n)) best = Math.min(best, 0)
      else best = Math.min(best, editDistance(key, n))
    }
    if (best <= Math.max(2, Math.floor(key.length / 3))) scored.push({ id: r.id, score: best })
  }
  return scored.sort((a, b) => a.score - b.score).slice(0, 4).map((s) => s.id)
}

/** The runtime for `--runtime <id|alias>`, or an error that lists close matches. */
export function runtimeOrThrow(id: string): RuntimeEntry {
  const r = getRuntime(id)
  if (r) return r
  const close = closeRuntimeIds(id)
  throw new BotStoreError(
    `unknown runtime '${id}'.` +
      (close.length > 0 ? ` Did you mean: ${close.join(", ")}?` : "") +
      " See `gizzi runtime catalog --all`.",
  )
}

/**
 * Whether a brain with this provider id may run on the runtime: the
 * registry's `allowed` (it fits, and a plan brain only where the runtime's
 * `plans` list that provider; a runtime's own CLI sign-in always passes).
 * Kept as a name for callers; the rule lives in runtime-registry.ts.
 */
export function brainFits(r: RuntimeEntry, provider: string): boolean {
  return allowed(r, provider)
}

/**
 * The first brain that fits (and is allowed on) a runtime with a CLI: its
 * CLI's default model, which is the runtime's own sign-in.
 */
export function firstFittingBrain(r: RuntimeEntry): string | undefined {
  const tool = r.tool ? cliCatalog().find((t) => t.id === r.tool) : undefined
  const model = tool?.models?.find((m) => m.default) ?? tool?.models?.[0]
  return tool && model ? `${tool.id}/${model.id}` : undefined
}

function brainProvider(brain: string): string {
  const slash = brain.indexOf("/")
  if (slash <= 0 || slash === brain.length - 1) {
    throw new BotStoreError(`--brain must be provider/model, e.g. claude-cli/claude-sonnet-5-5 (got '${brain}')`)
  }
  return brain.slice(0, slash)
}

/** `access` when the brain says it plainly: a CLI sign-in, a local model, or Allternit credits. */
function deriveAccess(r: RuntimeEntry, provider: string): AccessKind | undefined {
  let access: AccessKind | undefined
  if (provider === "allternit" || provider === "allternit-cloud") access = "credits"
  else if (cliCatalog().some((t) => t.id === provider || (t.aliases ?? []).includes(provider))) access = "plan"
  else if (["ollama", "lmstudio", "llamacpp"].includes(provider)) access = "local"
  else if (r.adapter.kind === "native") access = "key"
  return access && r.access.includes(access) ? access : undefined
}

/** `--brain`, with `--model` as its alias. Both given and different is an error. */
export function pickedBrain(args: { brain?: unknown; model?: unknown }): string | undefined {
  const brain = args.brain as string | undefined
  const model = args.model as string | undefined
  if (brain !== undefined && model !== undefined && brain !== model) {
    throw new BotStoreError(`--brain '${brain}' and --model '${model}' name different brains; pass one`)
  }
  return brain ?? model
}

export interface BrainChange {
  /** New bot.model (the legacy pin, kept equal to modelRef); undefined = unchanged. */
  model?: string | null
  /** New botBrain record; undefined = unchanged. */
  botBrain?: BotBrain
  /** The one-line auto-switch note, when the brain changed to fit the runtime. */
  note?: string
}

/**
 * Apply `--runtime` / `--brain` to a bot's record (contract: Compatibility).
 * `brain` "" clears the pin. With no runtime picked now or before, a brain is
 * only the legacy model pin (old `--model` behaviour).
 */
export function applyRuntimeBrain(input: {
  prev?: BotBrain | null
  currentModel?: string | null
  runtime?: string
  brain?: string
}): BrainChange {
  const { prev, currentModel, runtime: runtimeArg, brain: brainArg } = input
  if (runtimeArg === undefined && brainArg === undefined) return {}
  const runtime = runtimeArg !== undefined ? runtimeOrThrow(runtimeArg) : prev?.runtime ? getRuntime(prev.runtime) : undefined
  if (!runtime) {
    // No runtime anywhere: --brain/--model is the plain model pin, as before.
    return { model: brainArg === undefined ? undefined : brainArg || null }
  }

  let brain: string | undefined = brainArg !== undefined ? brainArg || undefined : (currentModel ?? undefined)
  let autoSwitchedFrom: string | undefined
  let note: string | undefined
  if (brain && !fits(runtime, brainProvider(brain))) {
    const fallback = firstFittingBrain(runtime)
    const only = runtime.brains === "any" ? "" : runtime.brains.map(providerName).join(" or ")
    if (!fallback) {
      throw new BotStoreError(
        `${runtime.name} runs ${only} models only, and gizzi can't pick one for you. Pass --brain <provider>/<model>.`,
      )
    }
    autoSwitchedFrom = brain
    brain = fallback
    note = `${runtime.name} runs ${only} models only, so the brain changed to ${fallback}.`
  } else if (brain && !brainFits(runtime, brainProvider(brain))) {
    // It fits, but it is a plan login this runtime may not use (contract:
    // plan_not_allowed_on_runtime). A brain picked now is refused; a brain
    // carried over from before switches to the runtime's own CLI model, or is
    // cleared when the runtime has no CLI of its own.
    const refusal = planRefusal(runtime, brainProvider(brain)) ?? `${runtime.name} can't use this brain.`
    if (brainArg) throw new BotStoreError(refusal)
    const fallback = firstFittingBrain(runtime)
    autoSwitchedFrom = brain
    brain = fallback
    note = fallback
      ? `${refusal} The brain changed to ${fallback}.`
      : `${refusal} The brain was cleared; pick one with --brain <provider>/<model>.`
  }

  const sameRuntime = prev?.runtime === runtime.id
  const {
    mode: _mode,
    harness: _harness,
    uhpHarnessId: _uhp,
    modelRef: _modelRef,
    access: _access,
    autoSwitchedFrom: _switched,
    nativeSessionId,
    ...kept
  } = (prev ?? {}) as BotBrain & { nativeSessionId?: string }
  const base = {
    ...kept,
    // A native session belongs to the runtime that made it.
    ...(sameRuntime && typeof nativeSessionId === "string" ? { nativeSessionId } : {}),
    runtime: runtime.id,
  }
  // resolveRuntime recomputes mode/harness/uhpHarnessId from the runtime.
  const next = resolveRuntime(base as unknown as BotBrainLike) as unknown as BotBrain
  if (brain) {
    next.modelRef = brain
    const access = deriveAccess(runtime, brainProvider(brain))
    if (access) next.access = access
  }
  if (autoSwitchedFrom) next.autoSwitchedFrom = autoSwitchedFrom
  const modelChanged = brainArg !== undefined || autoSwitchedFrom !== undefined
  return { botBrain: next, model: modelChanged ? (brain ?? null) : undefined, note }
}

/**
 * The vendor's name when this bot is a Connected bot (its platform agent has
 * a vendor execution binding: GET /agents/:id says `botKind: "connected"`),
 * else null. A bot with no platform link, or no answer from the platform
 * (signed out, offline), is not treated as Connected.
 */
export async function connectedVendor(bot: Bot): Promise<string | null> {
  const id = bot.platform?.id
  if (!id) return null
  const path = `/api/v1/agents/${encodeURIComponent(id)}`
  try {
    const res = await platformRequest<{ agent?: { botKind?: string } }>("GET", path, undefined, { timeoutMs: 10_000 })
    if (res?.agent?.botKind !== "connected") return null
  } catch {
    return null
  }
  try {
    const res = await platformRequest<{ binding?: { vendor?: string | null } }>(
      "GET",
      `/api/v1/gateway/bots/${encodeURIComponent(id)}/execution-binding`,
      undefined,
      { timeoutMs: 10_000 },
    )
    const vendor = res?.binding?.vendor
    if (vendor) return getRuntime(vendor)?.vendor ?? providerName(vendor)
  } catch {
    // fall through to the generic name
  }
  return "Its vendor"
}

function printBrainChange(change: BrainChange): void {
  if (change.note) UI.println(UI.Style.TEXT_WARNING + change.note + UI.Style.RESET)
}

/* -------------------------------------------------------------------------- */
/* create                                                                     */
/* -------------------------------------------------------------------------- */

export const BotCreateCommand = cmd({
  command: "create <name>",
  describe: "Create a bot profile (bot.json + SOUL.md + memory/)",
  builder: (yargs) =>
    yargs
      .positional("name", {
        describe: "bot name (lowercase slug: [a-z0-9-])",
        type: "string",
        demandOption: true,
      })
      .option("title", {
        type: "string",
        describe: "display title for the bot",
      })
      .option("description", {
        type: "string",
        describe: "what the bot is for",
      })
      .option("runtime", {
        type: "string",
        describe: "runtime id or alias the bot runs on (see `gizzi runtime catalog`), e.g. claude, codex, allternit",
      })
      .option("brain", {
        type: "string",
        describe: "brain (provider/model), e.g. claude-cli/claude-sonnet-5-5",
      })
      .option("model", {
        type: "string",
        describe: "same as --brain (kept for older scripts)",
      })
      .option("soul", {
        type: "string",
        describe: "path to a SOUL.md file to seed the persona (default: starter template)",
      })
      .example("gizzi agents bot create research-buddy --title 'Research Buddy'", "create a bot")
      .example(
        "gizzi agents bot create reviewer --model claude-cli/claude-sonnet-5-5 --soul ./SOUL.md",
        "with a model pin and custom persona",
      )
      .example("gizzi agents bot create coder --runtime codex", "run the bot on Codex with its default brain"),
  handler: async (args) => {
    await guard(async () => {
      const name = args.name as string
      let soul: string | undefined
      const soulPath = args.soul as string | undefined
      if (soulPath) {
        if (!existsSync(soulPath)) {
          throw new BotStoreError(`--soul file not found: ${soulPath}`)
        }
        soul = await readFile(soulPath, "utf8")
      }
      const brain = pickedBrain(args)
      const change = applyRuntimeBrain({ runtime: args.runtime as string | undefined, brain })
      const bot = await createBot({
        name,
        title: (args.title as string | undefined) ?? name,
        description: args.description as string | undefined,
        model: change.model !== undefined ? change.model : brain,
        botBrain: change.botBrain,
        soul,
      })
      printBrainChange(change)
      UI.println(
        UI.Style.TEXT_SUCCESS + `🤖 Bot '${bot.name}' created` + UI.Style.RESET,
      )
      if (bot.botBrain?.runtime) UI.println(`  Runtime: ${getRuntime(bot.botBrain.runtime)?.name ?? bot.botBrain.runtime}`)
      if (bot.model) UI.println(`  Brain: ${bot.model}`)
      UI.println(`  Home: ${botDir(bot.name)}`)
      UI.println("  Edit SOUL.md to give the bot a persona, then `gizzi agents bot chat " + bot.name + "`.")
      exitOk()
    })
  },
})

/* -------------------------------------------------------------------------- */
/* list                                                                       */
/* -------------------------------------------------------------------------- */

export const BotListCommand = cmd({
  command: ["list", "ls"],
  describe: "List bot profiles",
  builder: (yargs) => yargs,
  handler: async () => {
    await guard(async () => {
      const bots = await listBots()
      if (bots.length === 0) {
        UI.println("No bots yet.")
        UI.println("Create one with: gizzi agents bot create <name>")
        exitOk()
      }
      const rows = bots.map((bot) => [
        bot.name,
        bot.title,
        bot.model ?? "—",
        bot.canonicalSession ? `pinned:${bot.canonicalSession.sessionId.slice(0, 8)}` : "—",
      ])
      const widths = [0, 1, 2].map((i) =>
        Math.max(...rows.map((r) => r[i].length), ["NAME", "TITLE", "MODEL"][i].length),
      )
      const line = (cols: string[]) =>
        cols.map((c, i) => c.padEnd(widths[i])).join("  ").trimEnd()
      UI.println(UI.Style.TEXT_INFO_BOLD + line(["NAME", "TITLE", "MODEL", "CHAT"]) + UI.Style.RESET)
      for (const row of rows) UI.println(line(row))
      exitOk()
    })
  },
})

/* -------------------------------------------------------------------------- */
/* show                                                                       */
/* -------------------------------------------------------------------------- */

export const BotShowCommand = cmd({
  command: "show <name>",
  describe: "Show a bot's full identity, SOUL preview, and memory note count",
  builder: (yargs) =>
    yargs.positional("name", {
      describe: "bot name",
      type: "string",
      demandOption: true,
    }),
  handler: async (args) => {
    await guard(async () => {
      const bot = await getBot(args.name as string)
      if (!bot) {
        throw new BotStoreError(`bot '${args.name}' not found`)
      }
      await printBot(bot)
      exitOk()
    })
  },
})

async function printBot(bot: Bot): Promise<void> {
  const soul = await readSoul(bot.name)
  const memory = await listBotMemory(bot.name)
  UI.println(UI.Style.TEXT_INFO_BOLD + `Bot: ${bot.name}` + UI.Style.RESET)
  UI.println(`  Title:             ${bot.title}`)
  UI.println(`  Description:       ${bot.description || "—"}`)
  UI.println(`  Model:             ${bot.model ?? "—"}`)
  if (bot.botBrain?.runtime) {
    const r = getRuntime(bot.botBrain.runtime)
    UI.println(`  Runtime:           ${r ? `${r.name} (${r.id})` : bot.botBrain.runtime}`)
    if (bot.botBrain.autoSwitchedFrom) UI.println(`  Brain switched from: ${bot.botBrain.autoSwitchedFrom}`)
  }
  UI.println(`  Avatar:            ${bot.avatar ? bot.avatar.kind : "—"}`)
  UI.println(
    `  Canonical session: ${
      bot.canonicalSession
        ? `${bot.canonicalSession.sessionId} @ ${bot.canonicalSession.projectPath}`
        : "not pinned (lands in Phase B2)"
    }`,
  )
  UI.println(`  Capability epoch:  ${bot.capabilityEpoch ?? "—"}`)
  UI.println(`  Created:           ${bot.createdAt}`)
  UI.println(`  Updated:           ${bot.updatedAt}`)
  UI.println(`  Home:              ${botDir(bot.name)}`)
  UI.println(`  Memory notes:      ${memory.length}`)
  UI.println(`  SOUL.md (${soul ? soul.split("\n").length : 0} lines):`)
  if (soul) {
    const preview = soul.split("\n").slice(0, 12)
    for (const lineText of preview) UI.println(`    ${lineText}`)
    if (soul.split("\n").length > preview.length) {
      UI.println("    …")
    }
  } else {
    UI.println("    (missing)")
  }
}

/* -------------------------------------------------------------------------- */
/* edit                                                                       */
/* -------------------------------------------------------------------------- */

export const BotEditCommand = cmd({
  command: "edit <name>",
  describe: "Edit a bot's identity metadata (title / description) and its runtime and brain",
  builder: (yargs) =>
    yargs
      .positional("name", {
        describe: "bot name",
        type: "string",
        demandOption: true,
      })
      .option("title", {
        type: "string",
        describe: "new display title",
      })
      .option("description", {
        type: "string",
        describe: "new description",
      })
      .option("runtime", {
        type: "string",
        describe: "runtime id or alias to run on (see `gizzi runtime catalog`)",
      })
      .option("brain", {
        type: "string",
        describe: "new brain (provider/model); pass an empty string to clear",
      })
      .option("model", {
        type: "string",
        describe: "same as --brain (kept for older scripts)",
      }),
  handler: async (args) => {
    await guard(async () => {
      const name = args.name as string
      const brain = pickedBrain(args)
      const runtime = args.runtime as string | undefined
      const hasPatch =
        args.title !== undefined ||
        args.description !== undefined ||
        brain !== undefined ||
        runtime !== undefined
      if (!hasPatch) {
        throw new BotStoreError("nothing to update — pass at least one of --title, --description, --runtime, --brain")
      }
      let change: BrainChange = {}
      if (brain !== undefined || runtime !== undefined) {
        const current = await getBot(name)
        if (!current) throw new BotStoreError(`bot '${name}' not found`)
        const vendor = await connectedVendor(current)
        if (vendor) throw new BotStoreError(`This is a Connected bot. ${vendor} runs its runtime and brain.`)
        change = applyRuntimeBrain({ prev: current.botBrain, currentModel: current.model, runtime, brain })
      }
      const bot = await updateBot(name, {
        title: args.title as string | undefined,
        description: args.description as string | undefined,
        model: change.model,
        botBrain: change.botBrain,
      })
      printBrainChange(change)
      UI.println(UI.Style.TEXT_SUCCESS + `✓ Bot '${bot.name}' updated` + UI.Style.RESET)
      UI.println(`  Title: ${bot.title}`)
      if (bot.botBrain?.runtime) UI.println(`  Runtime: ${getRuntime(bot.botBrain.runtime)?.name ?? bot.botBrain.runtime}`)
      UI.println(`  Brain: ${bot.model ?? "—"}`)
      exitOk()
    })
  },
})

/* -------------------------------------------------------------------------- */
/* clone                                                                      */
/* -------------------------------------------------------------------------- */

export const BotCloneCommand = cmd({
  command: "clone <source> <new-name>",
  describe: "Clone a bot (identity, SOUL.md, memory/) — never its canonical session",
  builder: (yargs) =>
    yargs
      .positional("source", {
        describe: "existing bot name",
        type: "string",
        demandOption: true,
      })
      .positional("new-name", {
        describe: "name for the clone (lowercase slug)",
        type: "string",
        demandOption: true,
      }),
  handler: async (args) => {
    await guard(async () => {
      const clone = await cloneBot(args.source as string, args["new-name"] as string)
      const memoryCount = (await listBotMemory(clone.name)).length
      UI.println(
        UI.Style.TEXT_SUCCESS + `✓ Bot '${clone.name}' cloned from '${args.source}'` + UI.Style.RESET,
      )
      UI.println(`  Home: ${botDir(clone.name)}`)
      UI.println(`  Memory notes copied: ${memoryCount}`)
      UI.println("  Canonical session: not pinned (clones never inherit it)")
      exitOk()
    })
  },
})

/* -------------------------------------------------------------------------- */
/* delete                                                                     */
/* -------------------------------------------------------------------------- */

export const BotDeleteCommand = cmd({
  command: ["delete <name>", "rm <name>"],
  describe: "Delete a bot profile (bot.json, SOUL.md, memory/)",
  builder: (yargs) =>
    yargs
      .positional("name", {
        describe: "bot name",
        type: "string",
        demandOption: true,
      })
      .option("yes", {
        alias: "y",
        type: "boolean",
        describe: "skip the confirmation prompt",
        default: false,
      }),
  handler: async (args) => {
    await guard(async () => {
      const name = args.name as string
      const bot = await getBot(name)
      if (!bot) {
        throw new BotStoreError(`bot '${name}' not found`)
      }
      if (!args.yes) {
        if (!process.stdin.isTTY) {
          // clack prompts never resolve without a TTY — refuse instead of hanging.
          throw new BotStoreError(
            `refusing to prompt in a non-interactive shell — re-run with --yes to delete '${bot.name}'`,
          )
        }
        const confirmed = await prompts.confirm({
          message: `Delete bot '${bot.name}' (${bot.title}) and everything under ${botDir(bot.name)}?`,
          initialValue: false,
        })
        if (prompts.isCancel(confirmed) || !confirmed) {
          UI.println("Cancelled")
          exitOk()
        }
      }
      const removed = await deleteBot(bot.name)
      UI.println(
        UI.Style.TEXT_SUCCESS +
          `✓ Deleted bot '${removed.name}' — removed ${botDir(removed.name)} (bot.json, SOUL.md, memory/)` +
          UI.Style.RESET,
      )
      exitOk()
    })
  },
})

/* -------------------------------------------------------------------------- */
/* chat (B2 — canonical bot chat, D2/D6)                                      */
/* -------------------------------------------------------------------------- */

export const BotChatCommand = cmd({
  command: "chat <name> [message..]",
  describe: "Chat with a bot in its canonical session (created + pinned on first use)",
  builder: (yargs) =>
    yargs
      .positional("name", {
        describe: "bot name",
        type: "string",
        demandOption: true,
      })
      .positional("message", {
        describe: "message for the bot (omitted = interactive TUI)",
        type: "string",
        array: true,
      })
      .example("gizzi agents bot chat research-buddy", "open the bot's canonical chat in the TUI")
      .example("gizzi agents bot chat research-buddy 'summarize today's notes'", "one headless turn"),
  handler: async (args) => {
    await guard(async () => {
      const name = args.name as string
      const message = (args.message as string[] | undefined)?.join(" ")?.trim()
      const bot = await getBot(name)
      if (!bot) {
        throw new BotStoreError(`bot '${args.name}' not found`)
      }
      if (!message && !process.stdin.isTTY) {
        throw new BotStoreError(
          "no message given and stdin is not a terminal — usage: gizzi agents bot chat <name> [message..]",
        )
      }

      // Sessions belong under the project the bot was pinned in; the cwd is
      // the default project for a never-pinned bot.
      const projectPath = bot.canonicalSession?.projectPath ?? process.cwd()
      const opened = await bootstrap(projectPath, () => openCanonicalChat(bot, { projectPath }))
      const pinned = await getBot(bot.name)
      if (pinned) await ensureCapabilityEpoch(pinned)

      if (opened.created) {
        UI.println(
          UI.Style.TEXT_SUCCESS +
            `🤖 Canonical chat for '${bot.name}' created and pinned (session ${opened.sessionId}).` +
            UI.Style.RESET,
        )
      } else {
        UI.println(
          UI.Style.TEXT_DIM +
            `🤖 Resuming '${bot.name}' — canonical session ${opened.sessionId} @ ${opened.projectPath}` +
            UI.Style.RESET,
        )
      }

      if (!message) {
        // Interactive: the default command's -s/--session path. TuiThreadCommand
        // ends with process.exit(0) when the TUI closes.
        await (TuiThreadCommand.handler as any)?.({
          _: ["gizzi"],
          project: opened.projectPath,
          session: opened.sessionId,
          model: bot.model ?? undefined,
          "$0": "gizzi",
        })
        return
      }

      // Headless single turn into the canonical session, preserving continuity
      // (`gizzi run -s <id> --print` semantics). RunCommand self-exits in print
      // mode; UI banners above go to stderr so stdout stays pipe-clean.
      //
      // Typed failure taxonomy (B4/D4): print `[reason: <code>]` ahead of the
      // error text (Hermes format). Model errors surface as session.error bus
      // events — bus subscribers run synchronously at publish, before run.ts
      // renders the same event over SSE, so the reason line lands first.
      // Errors that reject the handler (bootstrap/prompt failures) are caught
      // and annotated below.
      let reasonPrinted = false
      let unsubscribe: () => void = () => {}
      try {
        // The failure-reason subscription and the headless run must both run
        // inside an Instance context: subscribing replays queued bus events,
        // and Session state (touched by subscribers) initializes lazily via
        // Instance.directory — outside a bootstrap() that throws
        // "No context found for instance" (B4 headless path).
        await bootstrap(projectPath, async () => {
          unsubscribe = Bus.subscribe(Session.Event.Error, (event) => {
            if (event.properties.sessionID !== opened.sessionId || reasonPrinted) return
            reasonPrinted = true
            UI.error(`[reason: ${classifyFailure(event.properties.error)}]`)
          })
          await (RunCommand.handler as any)?.({
            _: ["run"],
            message: [message],
            session: opened.sessionId,
            model: bot.model ?? undefined,
            print: true,
            outputFormat: "text",
            permissionMode: "dontAsk",
            "$0": "gizzi",
          })
        })
      } catch (e) {
        if (!reasonPrinted) {
          UI.error(`[reason: ${classifyFailure(e)}]`)
          reasonPrinted = true
        }
        throw e
      } finally {
        unsubscribe()
      }
    })
  },
})

/* -------------------------------------------------------------------------- */
/* routine (B3 — cron routines delivered into the canonical chat, D3)         */
/* -------------------------------------------------------------------------- */

export const BotRoutineAddCommand = cmd({
  command: "add <name>",
  describe: "Add a cron routine — delivers a prompt into the bot's canonical chat on schedule",
  builder: (yargs) =>
    yargs
      .positional("name", {
        describe: "bot name",
        type: "string",
        demandOption: true,
      })
      .option("label", {
        type: "string",
        describe: "routine label (default: the prompt text)",
      })
      .option("schedule", {
        type: "string",
        demandOption: true,
        describe: "cron expression or interval (e.g. 'daily at 9am', '*/15 * * * *')",
      })
      .option("prompt", {
        type: "string",
        demandOption: true,
        describe: "text delivered to the bot's canonical chat on each run",
      })
      .example(
        "gizzi agents bot routine add research-buddy --schedule 'daily at 9am' --prompt 'summarize new papers'",
        "daily routine for a bot",
      )
      .example(
        "gizzi agents bot routine add research-buddy --label 'paper sweep' --schedule '0 */6 * * *' --prompt '...'",
        "named routine on a cron expression",
      ),
  handler: async (args) => {
    await guard(async () => {
      const job = await addBotRoutine(args.name as string, {
        label: args.label as string | undefined,
        schedule: args.schedule as string,
        prompt: args.prompt as string,
      })
      UI.println(
        UI.Style.TEXT_SUCCESS +
          `✓ Routine added for bot '${args.name as string}'` +
          UI.Style.RESET,
      )
      UI.println(`  Job:      ${job.name} [${job.id}]`)
      UI.println(`  Schedule: ${describeSchedule(job.schedule)}`)
      UI.println(`  Bot:      ${(job.config as { bot?: string }).bot ?? "—"}`)
      UI.println("  Each run delivers the prompt into the bot's canonical chat.")
      exitOk()
    })
  },
})

export const BotRoutineListCommand = cmd({
  command: ["list <name>", "ls <name>"],
  describe: "List a bot's routines (schedule, status, next run)",
  builder: (yargs) =>
    yargs.positional("name", {
      describe: "bot name",
      type: "string",
      demandOption: true,
    }),
  handler: async (args) => {
    await guard(async () => {
      const jobs = await listBotRoutines(args.name as string)
      if (jobs.length === 0) {
        UI.println(`No routines for bot '${args.name as string}'.`)
        UI.println(
          "Add one with: gizzi agents bot routine add " + (args.name as string) + " --schedule <cron> --prompt <text>",
        )
        exitOk()
      }
      const rows = jobs.map((job) => [
        botRoutineLabel(job.name),
        describeSchedule(job.schedule),
        job.status,
        job.nextRunAt ?? "—",
      ])
      const headers = ["LABEL", "SCHEDULE", "STATUS", "NEXT RUN"]
      const widths = [0, 1, 2, 3].map((i) =>
        Math.max(...rows.map((r) => r[i].length), headers[i].length),
      )
      const line = (cols: string[]) =>
        cols.map((c, i) => c.padEnd(widths[i])).join("  ").trimEnd()
      UI.println(UI.Style.TEXT_INFO_BOLD + line(headers) + UI.Style.RESET)
      for (const row of rows) UI.println(line(row))
      exitOk()
    })
  },
})

export const BotRoutineRemoveCommand = cmd({
  command: ["remove <name> <jobId>", "rm <name> <jobId>"],
  describe: "Remove one of a bot's routines (refuses jobs owned by other bots)",
  builder: (yargs) =>
    yargs
      .positional("name", {
        describe: "bot name",
        type: "string",
        demandOption: true,
      })
      .positional("jobId", {
        describe: "cron job id (see `gizzi agents bot routine list <name>`)",
        type: "string",
        demandOption: true,
      }),
  handler: async (args) => {
    await guard(async () => {
      const job = await removeBotRoutine(args.name as string, args.jobId as string)
      UI.println(
        UI.Style.TEXT_SUCCESS +
          `✓ Removed routine '${job.name}' [${job.id}] from bot '${args.name as string}'` +
          UI.Style.RESET,
      )
      exitOk()
    })
  },
})

export const BotRoutineCommand = cmd({
  command: "routine",
  describe: "Bot routines — cron jobs that deliver prompts into a bot's canonical chat (D3)",
  builder: (yargs) =>
    yargs
      .command(BotRoutineAddCommand)
      .command(BotRoutineListCommand)
      .command(BotRoutineRemoveCommand)
      .demandCommand(1),
  handler: async () => {},
})

/* -------------------------------------------------------------------------- */
/* pause / resume (runtime-wide switch, allternit-api /api/v1/bots/pause)     */
/* -------------------------------------------------------------------------- */

async function runPause(args: { json?: boolean; status?: boolean }, target: boolean | "read") {
  await guard(async () => {
    const { getBotPause, setBotPause, describeBotPause } = await import("@/cli/work/api")
    const state = target === "read" ? await getBotPause() : await setBotPause(target)
    if (args.json) {
      process.stdout.write(JSON.stringify(state) + "\n")
    } else if (target === "read") {
      UI.println(describeBotPause(state))
    } else {
      UI.println(
        state.paused
          ? "All bots are paused. Routines and automatic replies are on hold; your own messages still run."
          : "Bots resumed.",
      )
    }
    exitOk()
  })
}

export const BotPauseCommand = cmd({
  command: "pause",
  describe: "Pause all bots: routines and automatic replies hold, your own messages still run",
  builder: (yargs) =>
    yargs
      .option("status", { type: "boolean", describe: "show the current state without changing it" })
      .option("json", { type: "boolean", describe: "print the runtime's JSON" }),
  handler: async (args) => runPause(args, args.status ? "read" : true),
})

export const BotResumeCommand = cmd({
  command: "resume",
  describe: "Resume all bots after a pause",
  builder: (yargs) => yargs.option("json", { type: "boolean", describe: "print the runtime's JSON" }),
  handler: async (args) => runPause(args, false),
})

/* -------------------------------------------------------------------------- */
/* group                                                                      */
/* -------------------------------------------------------------------------- */

export const BotCommand = cmd({
  command: "bot",
  describe: "hosted bots: profiles, chats, threads, routines (Bot Mode)",
  builder: (yargs) =>
    yargs
      .command(BotCreateCommand)
      .command(BotListCommand)
      .command(BotShowCommand)
      .command(BotEditCommand)
      .command(BotCloneCommand)
      .command(BotDeleteCommand)
      .command(BotChatCommand)
      .command(BotThreadsCommand)
      .command(BotRoutineCommand)
      .command(BotPauseCommand)
      .command(BotResumeCommand),
  handler: async () => {},
})

/**
 * Hidden top-level `gizzi bot pause|resume|pause --status`: the short form
 * the docs and phone apps name. Everything else about bots stays under
 * `gizzi agents bot`.
 */
export const BotPauseAliasCommand = cmd({
  command: "bot",
  describe: false,
  builder: (yargs) => yargs.command(BotPauseCommand).command(BotResumeCommand).demandCommand(1),
  handler: async () => {},
})
