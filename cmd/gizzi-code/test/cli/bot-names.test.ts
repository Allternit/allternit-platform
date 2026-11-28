import { expect, test } from "bun:test"
import { botNames } from "../../src/cli/work/bot-names"

test("botNames prefers displayName, then name, then the id", async () => {
  const m = await botNames(["a", "b", "c"], async () => ({
    agents: [{ id: "a", name: "alpha", botProfile: { displayName: "Alpha Bot" } }, { id: "b", name: "beta" }],
  }))
  expect([...m]).toEqual([["a", "Alpha Bot"], ["b", "beta"], ["c", "c"]])
})

test("botNames falls back to ids when the lookup fails", async () => {
  const m = await botNames(["x"], async () => { throw new Error("offline") })
  expect(m.get("x")).toBe("x")
})
