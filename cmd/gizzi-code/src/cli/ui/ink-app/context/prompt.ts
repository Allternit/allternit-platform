/**
 * Prompt-related constants and helpers used across the CLI.
 *
 * This used to be a Solid-JS context file; it is now a thin barrel that
 * re-exports the actual definitions from the tool prompt modules so the
 * production Ink/React TUI does not need Solid.
 */

export {
  BRIEF_TOOL_NAME,
  LEGACY_BRIEF_TOOL_NAME,
} from "../tools/BriefTool/prompt.js"

export { clearPromptCache } from "../tools/SkillTool/prompt.js"

export { isKairosCronEnabled } from "../tools/AutomationTool/prompt.js"

// HISTORY_SNIP is not implemented; keep a null placeholder so conditional
// requires of this module do not break at bundle time.
export const SNIP_TOOL_NAME: string | null = null
