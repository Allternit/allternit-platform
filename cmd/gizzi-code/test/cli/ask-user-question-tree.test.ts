import { describe, expect, test } from "bun:test"
import {
  expandQuestions,
  hasFollowUp,
  mergeAnswers,
  optionDescription,
  optionLabel,
} from "../../src/cli/ui/ink-app/components/permissions/AskUserQuestionPermissionRequest/decision-tree"

const incident = {
  question: "How should we stop the 502s?",
  header: "Incident",
  multiSelect: false,
  risk: "high" as const,
  context: "Checkout returns 502 for 4% of requests since deploy #482.",
  options: [
    {
      label: "Roll back to #481",
      description: "Back to the last good build.",
      recommended: true,
      then: {
        question: "Tell customers?",
        options: [
          { label: "Post on the status page", description: "Public note." },
          { label: "No notice", description: "Quiet fix." },
        ],
      },
    },
    { label: "Turn the flag off", description: "Disable the new path." },
  ],
}

describe("AskUserQuestion decision trees", () => {
  test("follow-up shows only after its branch is picked", () => {
    expect(expandQuestions([incident], {}).display).toHaveLength(1)
    expect(expandQuestions([incident], { [incident.question]: "Turn the flag off" }).display).toHaveLength(1)
    const { display } = expandQuestions([incident], { [incident.question]: "Roll back to #481" })
    expect(display.map((q) => q.question)).toEqual([incident.question, "Tell customers?"])
    expect(display[1].context).toBe('After "Roll back to #481"')
    expect(hasFollowUp(incident, "Roll back to #481")).toBe(true)
    expect(hasFollowUp(incident, "typed answer")).toBe(false)
  })

  test("answers fold into a path keyed by the root question", () => {
    expect(
      mergeAnswers([incident], { [incident.question]: "Roll back to #481", "Tell customers?": "Post on the status page" }),
    ).toEqual({ [incident.question]: "Roll back to #481 → Post on the status page" })
    expect(mergeAnswers([incident], { [incident.question]: "Turn the flag off", "Tell customers?": "stale" })).toEqual({
      [incident.question]: "Turn the flag off",
    })
  })

  test("a follow-up text that matches another question stays unique", () => {
    const other = { ...incident, question: "Tell customers?", options: incident.options.slice(1) }
    const { display } = expandQuestions([incident, other], { [incident.question]: "Roll back to #481" })
    expect(new Set(display.map((q) => q.question)).size).toBe(display.length)
  })

  test("labels mark the agent's pick and descriptions carry cautions", () => {
    expect(optionLabel(incident.options[0])).toBe("Roll back to #481 · Agent's pick")
    expect(optionDescription({ description: "Ship now.", caution: "Skips 1,284 tests" })).toBe(
      "Ship now. · ⚠ Skips 1,284 tests",
    )
  })
})
