import { describe, expect, test } from "bun:test"
import { Sleep } from "../../src/runtime/primitives/sleep"

describe("Sleep.wait", () => {
  test("wakes on the timer", async () => {
    const r = await Sleep.wait({ seconds: 1 })
    expect(r.woke).toBe("timer")
    expect(r.slept_seconds).toBeGreaterThanOrEqual(0.9)
  })

  test("wakes early on user input", async () => {
    let wake = () => {}
    let stopped = false
    const p = Sleep.wait({
      seconds: 60,
      watchUserInput: (w) => {
        wake = w
        return () => {
          stopped = true
        }
      },
    })
    setTimeout(() => wake(), 20)
    const r = await p
    expect(r.woke).toBe("user_input")
    expect(r.slept_seconds).toBeLessThan(5)
    expect(stopped).toBe(true)
  })

  test("wakes when cancelled, including before it starts", async () => {
    const ctl = new AbortController()
    const p = Sleep.wait({ seconds: 60, abort: ctl.signal })
    setTimeout(() => ctl.abort(), 20)
    expect((await p).woke).toBe("cancelled")
    expect((await Sleep.wait({ seconds: 60, abort: ctl.signal })).woke).toBe("cancelled")
  })

  test("clamps seconds to 1-900", () => {
    expect(Sleep.clampSeconds(0)).toBe(1)
    expect(Sleep.clampSeconds(5000)).toBe(900)
  })
})
