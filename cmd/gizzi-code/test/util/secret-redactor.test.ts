import { describe, expect, test } from "bun:test"
import fs from "fs"
import path from "path"
import {
  MASK,
  SecretRedactor,
  looksLikeSecret,
  maskCredentialFileLines,
} from "../../src/shared/util/secret-redactor"

// The vectors the Rust redactor (cmd/allternit-api/src/authority/redact.rs) is tested against too.
const VECTORS = path.resolve(import.meta.dir, "../../../../spec/Contracts/os/v1/conformance/redaction-vectors.json")
const doc = JSON.parse(fs.readFileSync(VECTORS, "utf8"))
const joined = (v: string | string[]) => (Array.isArray(v) ? v.join("") : v)

describe("secret redactor (AllternitOS step 10)", () => {
  test("uses the kernel's mask", () => {
    expect(doc.mask).toBe(MASK)
  })

  for (const v of doc.vectors) {
    test(`vector: ${v.name}`, () => {
      const r = new SecretRedactor(v.secrets)
      const input = joined(v.input)
      const out = v.mode === "credential_file" ? r.credentialFile(input) : r.text(input)
      for (const a of v.absent) expect(out).not.toContain(joined(a))
      for (const p of v.present) expect(out).toContain(joined(p))
      expect(r.text(out)).toBe(out)
    })
  }

  test("credential file lines keep their count and lose every value", () => {
    const lines = ["A=1234567", "-----BEGIN " + "RSA PRIVATE KEY-----", "MIIBOgIBAAJBAKTestOnlyBody", "-----END RSA PRIVATE KEY-----", "# note"]
    const out = maskCredentialFileLines(lines)
    expect(out.length).toBe(lines.length)
    expect(out.join("\n")).not.toContain("1234567")
    expect(out.join("\n")).not.toContain("MIIBOgIBAAJBAKTestOnlyBody")
    expect(out[4]).toBe("# note")
  })

  test("a handle is not a secret; a key is", () => {
    expect(looksLikeSecret("vault:github-deploy")).toBe(false)
    expect(looksLikeSecret("ghp" + "_TestOnlyNotARealToken000000000000abcd")).toBe(true)
  })
})
