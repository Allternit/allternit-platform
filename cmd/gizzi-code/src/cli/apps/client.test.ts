import { describe, expect, test } from "bun:test"
import { appLink, consentLines, formatApps, formatLinks, slugOf, validSlug } from "./client"
import { classifyTarget, LIVE_NOT_READY } from "./share-target"

describe("app helpers", () => {
  test("slugOf reads the shelf link; vendor apps have none", () => {
    expect(slugOf({ kind: "artifact", link: "/app/notes7" })).toBe("notes7")
    expect(slugOf({ kind: "vendor", link: "/app/vendor/vap_1" })).toBeUndefined()
    expect(slugOf({ kind: "artifact", link: "/app/Bad" })).toBeUndefined()
  })
  test("appLink and slug rule", () => {
    expect(appLink("notes7")).toBe("https://notes7.allternitusercontent.com")
    expect(validSlug("a1")).toBe(true)
    expect(validSlug("1a")).toBe(false)
    expect(validSlug("a".repeat(17))).toBe(false)
  })
  test("formatApps table", () => {
    const out = formatApps([
      { kind: "artifact", ref: "art_1", name: "Notes", provenance: "you", tier: "app", installed: true, link: "/app/notes7" },
    ])
    const [head, row] = out.split("\n")
    expect(head).toStartWith("NAME")
    expect(head).toContain("LEVEL")
    expect(row).toContain("notes7")
    expect(row).toContain("yes")
    expect(formatApps([])).toContain("No apps")
  })
  test("consentLines", () => {
    expect(consentLines({ requested_tier: "app", added_capabilities: ["storage", "ai"] })).toEqual([
      'This app asks for the "app" level.',
      "It can use: storage, ai",
    ])
  })
})

describe("share helpers", () => {
  test("classifyTarget", () => {
    expect(classifyTarget("3000")).toEqual({ kind: "port", port: 3000 })
    expect(classifyTarget("70000")).toEqual({ kind: "invalid" })
    expect(classifyTarget("notes7")).toEqual({ kind: "app", slug: "notes7" })
    expect(classifyTarget("Not Ok")).toEqual({ kind: "invalid" })
    expect(LIVE_NOT_READY).toContain("lane 39")
  })
  test("formatLinks table", () => {
    const out = formatLinks([
      { id: "shl_1", kind: "hosted", subject: "artifact", subdomain: "notes7", label: "Notes", status: "active", url: "https://notes7.allternitusercontent.com" },
      { id: "shl_2", kind: "live", subject: "port", subdomain: "x", target_port: 3000, status: "paused", url: null },
    ])
    expect(out).toContain("port 3000")
    expect(out).toContain("paused")
    expect(formatLinks([])).toBe("No share links.")
  })
})
