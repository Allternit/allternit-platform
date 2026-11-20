import { describe, expect, test } from "bun:test"
import { checkPublicKey, forwardSocketUrl, formatKeys, formatRuns, parseList, parsePort } from "../../src/cli/work/console-access"

describe("computers console access cli", () => {
  test("ports", () => {
    expect(parsePort("8080", "p")).toBe(8080)
    expect(() => parsePort("0", "remotePort")).toThrow("remotePort")
    expect(() => parsePort("70000", "p")).toThrow()
    expect(() => parsePort("80a", "p")).toThrow()
  })
  test("--on list", () => {
    expect(parseList("a, b,,a")).toEqual(["a", "b"])
    expect(parseList(undefined)).toEqual([])
  })
  test("public key check refuses private keys and junk", () => {
    expect(checkPublicKey("ssh-ed25519 AAAAC3Nza me@box\n")).toContain("ssh-ed25519")
    expect(() => checkPublicKey("-----BEGIN OPENSSH PRIVATE KEY-----")).toThrow("private")
    expect(() => checkPublicKey("hello")).toThrow()
  })
  test("forward socket url", () => {
    expect(forwardSocketUrl({ url: "/api/v1/x", ticket: "t1" }, "https://cloud.test")).toBe("wss://cloud.test/api/v1/x?ticket=t1")
    expect(forwardSocketUrl({ url: "wss://r.test/s" }, "https://cloud.test")).toBe("wss://r.test/s")
  })
  test("formatting", () => {
    expect(formatKeys([])).toBe("No SSH keys on this computer.")
    expect(formatKeys([{ id: "k1", label: "laptop", fingerprint: "SHA256:x" }])).toContain("never")
    expect(formatRuns([{ computerId: "c1", status: "pending", approvalId: "ap1" }], { c1: "box" })).toContain("approval ap1")
  })
})
