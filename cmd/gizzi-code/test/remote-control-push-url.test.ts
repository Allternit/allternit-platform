import { describe, expect, test } from "bun:test"
import { DEFAULT_PUSH_URL, pushWorkerUrlFrom } from "../src/runtime/integrations/remote-control-push"

// Regression: push was off in production because the URL had no default.
describe("remote control push URL", () => {
  test("unset uses the hosted worker", () => {
    expect(pushWorkerUrlFrom(undefined)).toBe(DEFAULT_PUSH_URL)
    expect(DEFAULT_PUSH_URL).toBe("https://push.fabrictransport.allternit.com")
  })
  test("an empty value turns push off", () => {
    expect(pushWorkerUrlFrom("")).toBeUndefined()
    expect(pushWorkerUrlFrom("  ")).toBeUndefined()
  })
  test("a configured worker wins", () => {
    expect(pushWorkerUrlFrom("https://push.example.test")).toBe("https://push.example.test")
  })
})
