import { describe, expect, test } from "bun:test"
import { Pairing } from "../../src/runtime/services/pairing/pairing"

describe("Pairing.approvalUrl", () => {
  test("sends platform.allternit.com/pair links to the working page on ai.allternit.com", () => {
    expect(Pairing.approvalUrl("https://platform.allternit.com/pair?code=YU8G-V9KQ")).toBe(
      "https://ai.allternit.com/pair?code=YU8G-V9KQ",
    )
  })

  test("leaves other links alone", () => {
    for (const url of ["https://ai.allternit.com/pair?code=A", "http://localhost:3013/pair?code=A", "https://platform.allternit.com/devices", "not a url"]) {
      expect(Pairing.approvalUrl(url)).toBe(url)
    }
  })
})

describe("Pairing.machineHostname", () => {
  test("on macOS it is the stable LocalHostName, not the network-assigned name", () => {
    const name = Pairing.machineHostname()
    expect(name.length).toBeGreaterThan(0)
    if (process.platform === "darwin") {
      const local = require("node:child_process").execFileSync("scutil", ["--get", "LocalHostName"], { encoding: "utf8" }).trim()
      expect(name).toBe(local)
    }
  })
})
