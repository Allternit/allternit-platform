import { Ripgrep } from "@/shared/file/ripgrep"

import { Instance } from "@/runtime/context/project/instance"
import { Filesystem } from "@/shared/util/filesystem"
import { getAutoMemPathFor } from "@/memdir/paths"

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
    // The memdir auto-memory directory — the same store the TUI memory UX
    // (`#` quick-add, /remember, extract-memories) reads and writes, resolved
    // through the shared memdir layer (src/memdir/paths).
    const memoryDir = getAutoMemPathFor(Instance.directory)
    const exists = await Filesystem.exists(memoryDir)
    return [
      `# auto memory`,
      ``,
      `You have a persistent, file-based memory system. Memory files persist across conversations and are automatically`,
      `loaded into future sessions. Your memory directory is \`${memoryDir}\` — the same auto-memory store the`,
      `interactive TUI uses, so memories are shared between headless and interactive sessions.`,
      ``,
      exists ? `The memory directory exists.` : `The memory directory does not exist yet — it will be created automatically when you save a memory.`,
      ``,
      `## How to save memories`,
      ``,
      `Use the \`memory_write\` tool (preferred) to save structured memories. Each memory has:`,
      `- **name**: short snake_case identifier (e.g. \`user_role\`, \`testing_feedback\`) — used as filename`,
      `- **description**: one-line summary — used to determine which memories are loaded in future sessions, so be specific`,
      `- **type**: \`user\` | \`feedback\` | \`project\` | \`reference\``,
      `- **body**: the memory content`,
      ``,
      `Types:`,
      `- \`user\` — user's role, preferences, expertise, communication style`,
      `- \`feedback\` — corrections or guidance from the user that should alter future behavior`,
      `- \`project\` — ongoing work context, goals, decisions, deadlines`,
      `- \`reference\` — pointers to external resources (URLs, Linear projects, dashboards)`,
      ``,
      `For \`feedback\` and \`project\` types, structure the body as: rule/fact first, then **Why:** and **How to apply:** lines.`,
      ``,
      `## How to recall memories`,
      ``,
      `Use the \`memory_recall\` tool to search across all saved memories before making assumptions.`,
      ``,
      `## Rules`,
      `- Do NOT save session-specific details, in-progress work state, or code derivable from the repo`,
      `- Do NOT save speculative or unverified conclusions from a single file`,
      `- Do NOT create duplicate memories — use \`memory_recall\` to check first, then update if one exists`,
      `- Update or remove memories that turn out to be wrong or outdated`,
      `- MEMORY.md is always loaded (lines after 200 are truncated) — topic files are loaded when relevant to the session`,
      ``,
      `## Explicit user requests`,
      `- "remember X" → use \`memory_write\` immediately`,
      `- "forget X" / "stop remembering X" → use \`memory_recall\` to find it, then use the write tool to remove or update it`,
      `- "correct" / "that's wrong" → update or remove the incorrect memory entry`,
    ].join("\n")
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
