import { Ripgrep } from "@/shared/file/ripgrep"

import { Instance } from "@/runtime/context/project/instance"
import { MemoryDrive } from "@/runtime/memory/drive/drive"

import PROMPT_DEFAULT from "@/runtime/session/prompt/default.txt"
import PROMPT_DEFAULT_WITHOUT_TODO from "@/runtime/session/prompt/qwen.txt"
import PROMPT_BEAST from "@/runtime/session/prompt/beast.txt"
import PROMPT_GEMINI from "@/runtime/session/prompt/gemini.txt"

import PROMPT_CODEX from "@/runtime/session/prompt/codex_header.txt"
import PROMPT_TRINITY from "@/runtime/session/prompt/trinity.txt"
import PROMPT_PLAN_MODE from "@/runtime/session/prompt/plan-mode.txt"
import PROMPT_BUILD_MODE from "@/runtime/session/prompt/build-mode.txt"
import PROMPT_MDX_GRAPHS from "@/runtime/session/prompt/mdx-graphs.txt"
import type { Provider } from "@/runtime/providers/provider"

export namespace SystemPrompt {
  export function instructions() {
    return PROMPT_CODEX.trim()
  }

  export function provider(model: Provider.Model, mode?: 'plan' | 'build') {
    const basePrompts = []
    
    // Add mode-specific prompt
    if (mode === 'plan') {
      basePrompts.push(PROMPT_PLAN_MODE)
    } else if (mode === 'build') {
      basePrompts.push(PROMPT_BUILD_MODE)
    }
    
    // Add provider-specific prompts
    if (model.api.id.includes("gpt-5")) basePrompts.push(PROMPT_CODEX)
    if (model.api.id.includes("gpt-") || model.api.id.includes("o1") || model.api.id.includes("o3"))
      basePrompts.push(PROMPT_BEAST)
    if (model.api.id.includes("gemini-")) basePrompts.push(PROMPT_GEMINI)
    if (model.api.id.includes("claude")) basePrompts.push(PROMPT_DEFAULT)
    if (model.api.id.toLowerCase().includes("trinity")) basePrompts.push(PROMPT_TRINITY)
    if (basePrompts.length === 0 || !basePrompts.includes(PROMPT_DEFAULT)) {
      basePrompts.push(PROMPT_DEFAULT_WITHOUT_TODO)
    }
    
    return basePrompts
  }

  async function memoryPrompt(): Promise<string> {
    // The user's Memory Drive (personal + mounted drives): each MEMORY.md
    // (bounded), its checkout path for reading topic files with normal
    // tools, and how to save with memory_write. Waits briefly for the
    // session-start sync, never blocks on the network.
    const context = await MemoryDrive.sessionContext({ writeTool: "memory_write" }).catch(() => "")
    if (context) {
      return [
        context,
        "",
        "## Rules",
        "- Use `memory_recall` before assuming a preference or convention, and to find a memory's id before updating it",
        "- \"remember X\" → `memory_write` immediately; \"forget X\" → find it with `memory_recall`, then `memory_write` with action delete",
        "- Do NOT save session-specific details, in-progress work state, code derivable from the repo, or unverified conclusions",
        "- Fix or delete memories that turn out to be wrong",
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
        `<directories>`,
        `  ${
          project.vcs === "git" && false
            ? await Ripgrep.tree({
                cwd: Instance.directory,
                limit: 50,
              })
            : ""
        }`,
        `</directories>`,
      ].join("\n"),
      await memoryPrompt(),
      PROMPT_MDX_GRAPHS.trim(),
    ]
  }
}
