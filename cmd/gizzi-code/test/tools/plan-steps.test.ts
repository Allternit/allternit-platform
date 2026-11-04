import { describe, expect, test } from "bun:test"
import { Question } from "../../src/runtime/integrations/question"
import { approvedStepsText, stepsFromMarkdown } from "../../src/runtime/tools/builtins/plan"

describe("plan steps", () => {
  test("numbered items are the steps, bullets elsewhere are ignored", () => {
    const md = "# Plan\n\n1. Read the routes\n2. **Add** the endpoint\n   - nested detail\n\n## Risks\n- Breaks old clients\n"
    expect(stepsFromMarkdown(md).map((s) => s.title)).toEqual(["Read the routes", "Add the endpoint"])
  })
  test("bullet checklists work when nothing is numbered; prose gives none", () => {
    expect(stepsFromMarkdown("- [ ] One\n- [x] Two\n").map((s) => s.title)).toEqual(["One", "Two"])
    expect(stepsFromMarkdown("Just do the thing.\n- only one")).toEqual([])
  })
  test("readPlan bounds and cleans a reply's plan", () => {
    expect(Question.readPlan([{ title: "  a  " }, { title: "" }, { title: "b", detail: "d" }])).toEqual([{ title: "a" }, { title: "b", detail: "d" }])
    expect(Question.readPlan("nope")).toBeUndefined()
    expect(Question.readPlan([1])).toBeUndefined()
    expect(Question.readPlan(Array.from({ length: 51 }, () => ({ title: "x" })))).toBeUndefined()
    expect(Question.readPlan([{ title: "x".repeat(900) }])![0].title.length).toBe(500)
  })
  test("approved text calls out edits", () => {
    expect(approvedStepsText([], false)).toBe("")
    expect(approvedStepsText([{ title: "A" }], true)).toContain("edited the plan")
    expect(approvedStepsText([{ title: "A" }], false)).toContain("1. A")
  })
})
