import { Instance } from "@/runtime/context/project/instance"
import { MemoryDrive } from "@/runtime/memory/drive/drive"

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
 *   then: addendum (gpt | gemini | open; none for claude ids), mode, voice, phone
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

  /** The turn's prompt context from the session surface and the user message's metadata. */
  export function context(surface: Session.Info["surface"], metadata?: Record<string, unknown>): PromptContext {
    return {
      surface,
      ...(metadata?.["reply_mode"] === "voice" ? { replyMode: "voice" as const } : {}),
      ...(metadata?.["form_factor"] === "phone" ? { formFactor: "phone" as const } : {}),
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

  /** Header for a normal (non-bot) turn. */
  export function provider(model: { api: { id: string } }, mode?: Mode, ctx: PromptContext = {}): string[] {
    const base = isChatSurface(ctx.surface)
      ? [PROMPT_BASE_CHAT, PROMPT_CAPABILITIES, PROMPT_SAFETY]
      : [PROMPT_BASE_CODE, PROMPT_SAFETY]
    const fam = family(model)
    return [...base, ...(fam ? [ADDENDA[fam]] : []), ...modePrompt(mode), ...signals(ctx)]
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
      ...(isChatSurface(ctx.surface) ? [PROMPT_CAPABILITIES] : []),
      PROMPT_SAFETY,
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

  export async function environment(model: Provider.Model) {
    const project = Instance.project
    return [
      [
        `You are powered by the model named ${model.api.id}. The exact model ID is ${model.providerID}/${model.api.id}`,
        `Here is some useful information about the environment you are running in:`,
        `<env>`,
        `  Working directory: ${Instance.workdir}`,
        `  Is directory a git repo: ${project.vcs === "git" ? "yes" : "no"}`,
        `  Platform: ${process.platform}`,
        `  Today's date: ${new Date().toDateString()}`,
        `</env>`,
      ].join("\n"),
      await memoryPrompt(),
      PROMPT_MDX_GRAPHS.trim(),
    ]
  }
}
