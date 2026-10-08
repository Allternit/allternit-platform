import { describe, expect, test } from "bun:test"
import { desktopApiBase } from "@/runtime/tools/builtins/desktop"
import { platformApiBase } from "@/runtime/bots/platform-api"

describe("desktop tool API base", () => {
  test("defaults to the shared Allternit API base, never the mail host", () => {
    const base = desktopApiBase({})
    expect(base).toBe(platformApiBase())
    expect(base).not.toContain("mail.news")
  })

  test("an explicit gateway override wins and loses its trailing slash", () => {
    expect(desktopApiBase({ ALLTERNIT_GATEWAY_URL: "https://example.test/" })).toBe("https://example.test")
    expect(desktopApiBase({ VITE_ALLTERNIT_GATEWAY_URL: "https://vite.test" })).toBe("https://vite.test")
  })
})
