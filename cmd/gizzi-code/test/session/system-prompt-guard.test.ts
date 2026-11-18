import { describe, expect, test } from "bun:test"
import fs from "node:fs"
import path from "node:path"

// W3d-4 guard: the same vendor-name and "Allternit Shell" rule as
// system-prompt-selection.test.ts, applied to the TUI prompt stack and to every
// built-in agent prompt (runtime copy and TUI copy). Plus the injection wording.

const SRC = path.join(import.meta.dir, "../../src")
const VENDOR = /claude|anthropic|chatgpt|openai|gemini|grok|codex|opencode|duet|allternit shell/i

const TUI = "cli/ui/ink-app"
const RUNTIME_AGENTS = "runtime/tools/builtins/AgentTool/built-in"
const TUI_AGENTS = `${TUI}/tools/AgentTool/built-in`

const listed = [
  `${TUI}/constants/prompts.ts`,
  `${TUI}/constants/systemPromptSections.ts`,
  `${RUNTIME_AGENTS}/exploreAgent.ts`,
  `${RUNTIME_AGENTS}/planAgent.ts`,
  `${RUNTIME_AGENTS}/verificationAgent.ts`,
  `${TUI_AGENTS}/exploreAgent.ts`,
  `${TUI_AGENTS}/planAgent.ts`,
  `${TUI_AGENTS}/verificationAgent.ts`,
  `${TUI_AGENTS}/generalPurposeAgent.ts`,
  `${TUI_AGENTS}/catalogAgents.ts`,
  "runtime/agents/builtin-catalog.ts",
  "coordinator/coordinatorMode.ts",
  `${TUI}/coordinator/coordinatorMode.ts`,
  "cli/mission/orchestrator.ts",
  "runtime/server/routes/critique.ts",
]

// Source lines that are code, not prompt text, and may name a vendor on purpose.
// Each entry says why. Keep this list short.
const CODE_LINE_ALLOW: RegExp[] = [
  /canonical\.includes\(/, // model-id match for the knowledge cutoff, never sent as text
  /omitClaudeMd/, // legacy field name on the agent record
  /No brain configured|Provider-agnostic/, // critique setup error text, names providers the user may pick
]

// Whole files that teach a real outside name on purpose (a docs URL, a legacy
// project file, example model ids). They are checked by name, not by regex.
const EXEMPT = new Set([`${TUI_AGENTS}/gizziGuideAgent.ts`, `${TUI_AGENTS}/statuslineSetup.ts`])

function promptLines(rel: string) {
  const text = fs.readFileSync(path.join(SRC, rel), "utf8").replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\n]/g, ""))
  return text
    .split("\n")
    .map((line, i) => ({ line: i + 1, text: line }))
    .filter(({ text }) => !/^\s*\/\//.test(text))
    .filter(({ text }) => !CODE_LINE_ALLOW.some((rx) => rx.test(text)))
}

describe("W3d-4: vendor names and Allternit Shell in the TUI stack and built-in agent prompts", () => {
  test("every listed file exists", () => {
    for (const rel of listed) expect(fs.existsSync(path.join(SRC, rel))).toBe(true)
  })

  test("both copies of the built-in agent prompts are covered", () => {
    for (const name of ["exploreAgent.ts", "planAgent.ts", "verificationAgent.ts"]) {
      expect(listed).toContain(`${RUNTIME_AGENTS}/${name}`)
      expect(listed).toContain(`${TUI_AGENTS}/${name}`)
    }
  })

  for (const rel of listed) {
    test(rel, () => {
      const hits = promptLines(rel).filter(({ text }) => VENDOR.test(text))
      expect(hits).toEqual([])
    })
  }

  test("exempt files stay on the short list", () => {
    expect([...EXEMPT].sort()).toEqual([`${TUI_AGENTS}/gizziGuideAgent.ts`, `${TUI_AGENTS}/statuslineSetup.ts`])
    for (const rel of EXEMPT) expect(fs.existsSync(path.join(SRC, rel))).toBe(true)
  })
})

describe("W3d-4: injection wording", () => {
  const read = (rel: string) => fs.readFileSync(path.join(SRC, rel), "utf8")

  test("the TUI system prompt builds on the shared code base and safety text", () => {
    const text = read(`${TUI}/constants/prompts.ts`)
    expect(text).toContain("@/runtime/session/prompt/base-code.txt")
    expect(text).toContain("@/runtime/session/prompt/safety.txt")
  })

  test("coordinator and catalog prompts say that worker, file and page text is data", () => {
    for (const rel of ["coordinator/coordinatorMode.ts", `${TUI}/coordinator/coordinatorMode.ts`, "runtime/agents/builtin-catalog.ts"]) {
      expect(read(rel)).toMatch(/\bis data\b|as data\b|never .*instructions/i)
    }
  })

  test("no prompt tells the model to ignore earlier instructions", () => {
    for (const rel of listed) {
      expect(promptLines(rel).filter(({ text }) => /ignore (all )?(previous|prior|above) instructions/i.test(text))).toEqual([])
    }
  })
})
