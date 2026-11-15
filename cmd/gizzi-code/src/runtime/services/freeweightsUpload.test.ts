import { describe, expect, test } from "bun:test"
import { chunk, commitBody, freeweightsKey, parseRepoId, storageLine } from "./freeweightsUpload"

describe("freeweightsKey", () => {
  test("takes only alt_ keys", () => {
    expect(freeweightsKey({ HF_TOKEN: "hf_abc", ALLTERNIT_API_KEY: "alt_1" } as any)).toBe("alt_1")
    expect(freeweightsKey({ HF_TOKEN: "hf_abc" } as any)).toBeUndefined()
  })
})
describe("parseRepoId", () => {
  test("accepts handle/name, rejects the rest", () => {
    expect(parseRepoId("eoj/tiny-1b")).toEqual({ namespace: "eoj", name: "tiny-1b" })
    expect(() => parseRepoId("tiny")).toThrow()
    expect(() => parseRepoId("a/b/c")).toThrow()
  })
})
describe("commitBody", () => {
  test("header first, one line per op, lfs carries oid and size", () => {
    const lines = commitBody("s", undefined, [
      { kind: "file", path: "README.md", base64: "aGk=" },
      { kind: "lfs", path: "w.gguf", oid: "ab", size: 5 },
    ]).trim().split("\n").map((l) => JSON.parse(l))
    expect(lines[0].key).toBe("header")
    expect(lines[1].value.encoding).toBe("base64")
    expect(lines[2]).toEqual({ key: "lfsFile", value: { path: "w.gguf", algo: "sha256", oid: "ab", size: 5 } })
  })
})
describe("chunk", () => {
  test("splits by the limit and never loops on a bad one", () => {
    expect(chunk([1, 2, 3, 4, 5], 2)).toEqual([[1, 2], [3, 4], [5]])
    expect(chunk([1, 2], 0)).toEqual([[1], [2]])
  })
})
describe("storageLine", () => {
  test("renders used and cap, flags paused, hides when unknown", () => {
    expect(storageLine({ user_cap_bytes: 5e8, used_bytes: 1e8 })).toBe("Freeweights storage: 100 MB of 500 MB")
    expect(storageLine({ user_cap_bytes: 5e8, uploads_open: false })).toContain("paused")
    expect(storageLine(undefined)).toBeUndefined()
  })
})
