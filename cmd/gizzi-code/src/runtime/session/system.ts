import { Instance } from "@/runtime/context/project/instance"
import { MemoryDrive } from "@/runtime/memory/drive/drive"
import { Clock } from "@/runtime/primitives/clock"

import PROMPT_BASE_CHAT from "@/runtime/session/prompt/base-chat.txt"
import PROMPT_BASE_CODE from "@/runtime/session/prompt/base-code.txt"
import PROMPT_CAPABILITIES from "@/runtime/session/prompt/capabilities.txt"
import PROMPT_SAFETY from "@/runtime/session/prompt/safety.txt"
import PROMPT_ADDENDUM_GPT from "@/runtime/session/prompt/addendum-gpt.txt"
import PROMPT_ADDENDUM_GEMINI from "@/runtime/session/prompt/addendum-gemini.txt"
import PROMPT_ADDENDUM_OPEN from "@/runtime/session/prompt/addendum-open.txt"
import PROMPT_VOICE from "@/runtime/session/prompt/voice.txt"
import PROMPT_PHONE from "@/runtime/session/prompt/phone.txt"
import PROMPT_PLAN_MODE from "@/runtime/session/prompt/plan-mode.txt"
import PROMPT_BUILD_MODE from "@/runtime/session/prompt/build-mode.txt"
import PROMPT_MDX_GRAPHS from "@/runtime/session/prompt/mdx-graphs.txt"
import PROMPT_BOT from "@/runtime/session/prompt/bot.txt"
import PROMPT_DUET from "@/runtime/session/prompt/duet.txt"
import PROMPT_DELEGATION from "@/runtime/session/prompt/delegation.txt"
import PROMPT_PROJECT_THREAD from "@/runtime/session/prompt/project-thread.txt"
import PROMPT_COMPUTER_USE from "@/runtime/session/prompt/computer-use.txt"
import type { Provider } from "@/runtime/providers/provider"
import type { Session } from "@/runtime/session"

/**
 * The runtime's own header prompts. The stack is chosen by the session's
 * surface (not the model id), then at most one addendum per model family,
 * then the mode reminder and the per-turn voice / phone blocks. The stable
 * parts come first so the cached prefix holds across turns.
 *
 *   chat, cowork, browser, design → base-chat, capabilities, safety
 *   code or unset (gizzi CLI)     → base-code, safety
 *   then: addendum (gpt | gemini | open; none for claude ids), mode,
 *         project-thread, computer-use, voice, phone
 *
 * Bot turns (`bot`): mode, bot, duet, delegation, project-thread,
 * capabilities (chat surfaces), safety, computer-use, voice, phone.
 *
 * The tool-signal blocks come from the turn's resolved tool set (already
 * gated in prompt.ts resolveTools), so a block is sent only when the model
 * has the tools it talks about: duet (`suggest_task` / `grant_task_permission`,
 * bot turns only), delegation (`project_send`, bot turns only),
 * project-thread (a project task thread) and computer-use (a browser or
 * computer tool).
 */
export namespace SystemPrompt {
  export type Mode = "plan" | "build"
  export type Surface = NonNullable<Session.Info["surface"]>
  export type PromptContext = {
    surface?: Session.Info["surface"]
    /** The turn's reply is read aloud (user metadata `reply_mode: "voice"`). */
    replyMode?: "voice"
    /** The user is on a phone (user metadata `form_factor: "phone"`). */
    formFactor?: "phone"
    /** An app bot's chat: the turn has `suggest_task` or `grant_task_permission` (duet.txt, bot turns only). */
    appBot?: boolean
    /** A bot's main chat that can send work to projects: the turn has `project_send` (delegation.txt, bot turns only). */
    projectSend?: boolean
    /** A project task thread (project-thread.txt). */
    projectThread?: boolean
    /** The turn has a browser or computer tool (computer-use.txt). */
    computerUse?: boolean
  }
  /** What the turn knows beyond its metadata: its resolved tool ids and the app bot caller (roster). */
  export type TurnSignals = {
    toolIds?: readonly string[]
    caller?: { projectId?: string | null; mainChat?: boolean } | null
  }
  export type Family = "gpt" | "gemini" | "open"

  const CHAT_SURFACES: ReadonlySet<string> = new Set<Surface>(["chat", "cowork", "browser", "design"])

  const ADDENDA: Record<Family, string> = {
    gpt: PROMPT_ADDENDUM_GPT,
    gemini: PROMPT_ADDENDUM_GEMINI,
    open: PROMPT_ADDENDUM_OPEN,
  }

  /** Chat-family surfaces get the chat stack; code or unset get the code stack. */
  export function isChatSurface(surface: Session.Info["surface"]): boolean {
    return surface !== undefined && CHAT_SURFACES.has(surface)
  }

  /**
   * The model's family for the addendum: "gpt" (gpt-*, o1/o3/o4), "gemini",
   * undefined for claude ids (no addendum), "open" for everything else
   * (qwen, kimi, glm, deepseek, trinity, llama, mistral, local, unknown).
   * A routed id ("openai/gpt-5", "anthropic/claude-…") is read by its last segment.
   */
  export function family(model: { api: { id: string } }): Family | undefined {
    const id = model.api.id.toLowerCase()
    const name = id.slice(id.lastIndexOf("/") + 1)
    if (name.includes("claude")) return undefined
    if (name.includes("gpt-") || /^o[134](-|$)/.test(name)) return "gpt"
    if (name.includes("gemini")) return "gemini"
    return "open"
  }

  /** Suggestions and grants (duet.txt): an app bot's own chat (prompt.ts gates both on isAppBotSession). */
  export const APP_BOT_TOOL_IDS: readonly string[] = ["suggest_task", "grant_task_permission"]
  /** Sending work to a project (delegation.txt): a bot's main chat (gated on isAppBotMainChat). */
  export const PROJECT_SEND_TOOL_ID = "project_send"
  /**
   * Browser and computer tools (computer-use.txt), by exact id. The computer,
   * computer_v2 and browser toolsets are one function tool each (members are
   * the `action` field, never their own tool id; computer-toolset/index.ts).
   */
  export const COMPUTER_TOOL_IDS: readonly string[] = ["pane_browser", "browser", "computer", "computer_v2", "computers"]

  /**
   * The turn's prompt context from the session surface, the user message's
   * metadata and the turn's signals: its resolved tool ids and, for app bot
   * chats, the roster caller (`appBotCaller`). A project thread is either a
   * caller with a project that is not a main chat, or the server's hint
   * `metadata.project_role: "thread"` (the internal project worker).
   */
  export function context(
    surface: Session.Info["surface"],
    metadata?: Record<string, unknown>,
    turn: TurnSignals = {},
  ): PromptContext {
    const ids = new Set(turn.toolIds ?? [])
    const has = (list: readonly string[]) => list.some((id) => ids.has(id))
    const caller = turn.caller
    const inThread =
      metadata?.["project_role"] === "thread" || (!!caller?.projectId && caller.mainChat !== true)
    return {
      surface,
      ...(metadata?.["reply_mode"] === "voice" ? { replyMode: "voice" as const } : {}),
      ...(metadata?.["form_factor"] === "phone" ? { formFactor: "phone" as const } : {}),
      ...(has(APP_BOT_TOOL_IDS) ? { appBot: true } : {}),
      ...(ids.has(PROJECT_SEND_TOOL_ID) ? { projectSend: true } : {}),
      ...(inThread ? { projectThread: true } : {}),
      ...(has(COMPUTER_TOOL_IDS) ? { computerUse: true } : {}),
    }
  }

  function modePrompt(mode?: Mode): string[] {
    if (mode === "plan") return [PROMPT_PLAN_MODE]
    if (mode === "build") return [PROMPT_BUILD_MODE]
    return []
  }

  function signals(ctx: PromptContext): string[] {
    return [
      ...(ctx.replyMode === "voice" ? [PROMPT_VOICE] : []),
      ...(ctx.formFactor === "phone" ? [PROMPT_PHONE] : []),
    ]
  }

  function projectThread(ctx: PromptContext): string[] {
    return ctx.projectThread ? [PROMPT_PROJECT_THREAD] : []
  }

  function computerUse(ctx: PromptContext): string[] {
    return ctx.computerUse ? [PROMPT_COMPUTER_USE] : []
  }

  /**
   * Header for a normal (non-bot) turn. Never the duet or delegation blocks:
   * those are bot rules, even when a normal turn has the tools.
   */
  export function provider(model: { api: { id: string } }, mode?: Mode, ctx: PromptContext = {}): string[] {
    const base = isChatSurface(ctx.surface)
      ? [PROMPT_BASE_CHAT, PROMPT_CAPABILITIES, PROMPT_SAFETY]
      : [PROMPT_BASE_CODE, PROMPT_SAFETY]
    const fam = family(model)
    return [
      ...base,
      ...(fam ? [ADDENDA[fam]] : []),
      ...modePrompt(mode),
      ...projectThread(ctx),
      ...computerUse(ctx),
      ...signals(ctx),
    ]
  }

  /**
   * Header for a bot turn (runtime/bots/bot-turn.ts): the mode reminder, if
   * any, then neutral operating guidance — never a base identity. The bot's
   * persona leads the system prompt ahead of this.
   */
  export function bot(mode?: Mode, ctx: PromptContext = {}): string[] {
    return [
      ...modePrompt(mode),
      PROMPT_BOT,
      ...(ctx.appBot ? [PROMPT_DUET] : []),
      ...(ctx.projectSend ? [PROMPT_DELEGATION] : []),
      ...projectThread(ctx),
      ...(isChatSurface(ctx.surface) ? [PROMPT_CAPABILITIES] : []),
      PROMPT_SAFETY,
      ...computerUse(ctx),
      ...signals(ctx),
    ]
  }

  /** The same stack as `provider`, as one string (Codex OAuth `options.instructions`). */
  export function instructions(model: { api: { id: string } }, mode?: Mode, ctx: PromptContext = {}): string {
    return provider(model, mode, ctx).join("\n\n")
  }

  /**
   * A session's surface for prompt choice: its own, else the nearest
   * ancestor's (child sessions are never stamped, so `isAppSession` stays as
   * it is). Walks at most 10 parents; a missing parent ends the walk.
   */
  export async function effectiveSurface(
    session: { surface?: Session.Info["surface"]; parentID?: string },
    getParent: (
      id: string,
    ) =>
      | Promise<{ surface?: Session.Info["surface"]; parentID?: string } | undefined>
      | { surface?: Session.Info["surface"]; parentID?: string }
      | undefined,
  ): Promise<Session.Info["surface"]> {
    let current: { surface?: Session.Info["surface"]; parentID?: string } | undefined = session
    for (let depth = 0; current && depth <= 10; depth++) {
      if (current.surface) return current.surface
      if (!current.parentID) return undefined
      const parentID: string = current.parentID
      current = await Promise.resolve()
        .then(() => getParent(parentID))
        .catch(() => undefined)
    }
    return undefined
  }

  async function memoryPrompt(): Promise<string> {
    // The user's Memory Drive (personal + mounted drives): each MEMORY.md
    // (bounded), its checkout path for reading topic files with normal
    // tools, and how to save with memory_write. Waits briefly for the
    // session-start sync, never blocks on the network. What to save and how
    // to use it lives in the base prompt; these are only the tool mechanics.
    const context = await MemoryDrive.sessionContext({ writeTool: "memory_write" }).catch(() => "")
    if (context) {
      return [
        context,
        "",
        "## Memory tools",
        "- Use `memory_recall` before assuming a preference or convention, and to find a memory's id before changing it",
        "- \"remember X\" → `memory_write`; \"forget X\" → find it with `memory_recall`, then `memory_write` with action delete",
      ].join("\n")
    }
    return "# Memory\n\nPersistent memory is turned off for this session (GIZZI_MEMORY_DRIVE=0)."
  }

  /**
   * The environment section. The date is the user's (the zone resolved by
   * Clock: this turn's `metadata.time_zone`, else the last client zone, else
   * this machine's), with a `Time zone:` line. Never the time of day, so the
   * cached prefix holds across turns; `current_time` gives the exact time.
   */
  export async function environment(model: Provider.Model, turn: { turnZone?: string } = {}) {
    const project = Instance.project
    const clock = await Clock.resolveTimeZone({ turnZone: turn.turnZone })
    const zoneNote = clock.source === "process" ? " (this machine's; may not be the user's)" : ""
    return [
      [
        `You are powered by the model named ${model.api.id}. The exact model ID is ${model.providerID}/${model.api.id}`,
        `Here is some useful information about the environment you are running in:`,
        `<env>`,
        `  Working directory: ${Instance.workdir}`,
        `  Is directory a git repo: ${project.vcs === "git" ? "yes" : "no"}`,
        `  Platform: ${process.platform}`,
        `  Today's date: ${Clock.dateString(clock.zone)}`,
        `  Time zone: ${clock.zone}${zoneNote}`,
        `</env>`,
      ].join("\n"),
      await memoryPrompt(),
      PROMPT_MDX_GRAPHS.trim(),
    ]
  }
}
