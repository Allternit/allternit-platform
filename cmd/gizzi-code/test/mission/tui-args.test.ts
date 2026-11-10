import { describe, expect, test } from "bun:test"
import { parseMissionArgs } from "../../src/cli/ui/ink-app/commands/mission/mission"

// Regression: /mission only knew --brain, so `--on`, `--max-workers` and
// `--max-hours` typed in the TUI became part of the goal the orchestrator planned.
describe("/mission arguments", () => {
  test("the CLI's flags are taken out of the goal, in either form and any position", () => {
    const a = parseMissionArgs('--max-workers 2 "port the parser" --on studio --max-hours=1.5 -m codex-cli/x --max-attempts 0')
    expect(a.goal).toBe("port the parser")
    expect(a).toMatchObject({ brain: "codex-cli/x", on: "studio", maxWorkers: 2, maxHours: 1.5, maxAttempts: 0 })
    expect(a.problem).toBeUndefined()
  })

  test("a goal that only mentions dashes is left alone", () => {
    expect(parseMissionArgs("add a --verbose flag to the cli").goal).toBe("add a --verbose flag to the cli")
  })

  test("a bad or missing value is reported, not planned", () => {
    expect(parseMissionArgs("x --max-workers two").problem).toContain("--max-workers")
    expect(parseMissionArgs("x --max-workers 0").problem).toContain("--max-workers")
    expect(parseMissionArgs("x --max-hours").problem).toBe("--max-hours needs a value")
  })
})
