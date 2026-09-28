import { describe, expect, test } from "bun:test"
import { parseResultBlock, subagentResult } from "../../src/runtime/tools/builtins/subagent-result"

describe("subagent result contract", () => {
  test("reads the trailing result block and drops it from the reply", () => {
    const text = 'Checked 3 hosts.\n\n```json\n{"result": {"summary": "Blended colo + power: $0.41/GPU-hr", "confidence": 0.87, "openQuestions": ["Cross-connect fees?", "Annual terms?"], "artifacts": ["colo.xlsx"]}}\n```'
    const { result, body } = parseResultBlock(text)
    expect(result).toEqual({ summary: "Blended colo + power: $0.41/GPU-hr", confidence: 0.87, openQuestions: ["Cross-connect fees?", "Annual terms?"], artifacts: ["colo.xlsx"] })
    expect(body).toBe("Checked 3 hosts.")
  })

  test("without a block it derives a card from the reply and the files written", () => {
    const messages = [{ info: {}, parts: [{ type: "tool", tool: "write", state: { status: "completed", input: { filePath: "/tmp/pricing.csv" } } }] }] as any
    const { result } = subagentResult("Lambda is $2.49/hr. CoreWeave is $2.23/hr. Should we include RunPod?", messages)
    expect(result.summary).toBe("Lambda is $2.49/hr. CoreWeave is $2.23/hr.")
    expect(result.openQuestions).toEqual(["Should we include RunPod?"])
    expect(result.artifacts).toEqual(["/tmp/pricing.csv"])
    expect(result.confidence).toBeUndefined()
  })

  test("a malformed block is left in the reply", () => {
    expect(parseResultBlock('Done.\n```json\n{"result": {"summary": ""}}\n```').result).toBeUndefined()
  })
})
