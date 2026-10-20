import { afterEach, describe, expect, test } from "bun:test"
import { DotEnv } from "../../src/runtime/session/environment/dotenv"
import { SecretRedactor } from "../../src/runtime/session/environment/redactor"
import { EgressProxy } from "../../src/runtime/session/environment/egress-proxy"

describe("DotEnv.parse", () => {
  test("plain, export, comments, quotes and multiline values", () => {
    const { vars, invalid } = DotEnv.parse(
      [
        "# comment",
        "A=1",
        "export B=two words   # trailing",
        'C="line1\\nline2"',
        "D='keep $HOME \\n'",
        'E="multi',
        'line"',
        "not a var",
        "1BAD=x",
      ].join("\n"),
    )
    expect(vars).toEqual({ A: "1", B: "two words", C: "line1\nline2", D: "keep $HOME \\n", E: "multi\nline" })
    expect(invalid.map((i) => i.line)).toEqual([8, 9])
  })
})

describe("SecretRedactor", () => {
  afterEach(() => SecretRedactor.unregister("s1"))

  test("replaces values and their base64 form, leaves others alone", () => {
    SecretRedactor.register("s1", { API_TOKEN: "sk-live-123456", PIN: "12" })
    const b64 = Buffer.from("sk-live-123456").toString("base64")
    expect(SecretRedactor.text(`token=sk-live-123456 b=${b64}`)).toBe("token=[secret:API_TOKEN] b=[secret:API_TOKEN]")
    // Too short to redact safely.
    expect(SecretRedactor.text("pin 12")).toBe("pin 12")
    const untouched = { a: "nothing here" }
    expect(SecretRedactor.deep(untouched)).toBe(untouched)
    expect(SecretRedactor.deep({ a: ["x sk-live-123456"] })).toEqual({ a: ["x [secret:API_TOKEN]"] })
  })

  test("inactive once unregistered", () => {
    SecretRedactor.register("s1", { K: "abcdefgh" })
    SecretRedactor.unregister("s1")
    expect(SecretRedactor.active()).toBe(false)
    expect(SecretRedactor.text("abcdefgh")).toBe("abcdefgh")
  })
})

describe("EgressProxy host patterns", () => {
  test("exact, wildcard, dot-prefix and star", () => {
    expect(EgressProxy.matches("github.com", ["github.com"])).toBe(true)
    expect(EgressProxy.matches("api.github.com", ["github.com"])).toBe(false)
    expect(EgressProxy.matches("api.github.com", ["*.github.com"])).toBe(true)
    expect(EgressProxy.matches("github.com", ["*.github.com"])).toBe(false)
    expect(EgressProxy.matches("github.com", [".github.com"])).toBe(true)
    expect(EgressProxy.matches("evil.com", ["*"])).toBe(true)
    expect(EgressProxy.normalize(["https://Registry.npmjs.org/foo", "pypi.org:443"])).toEqual(["registry.npmjs.org", "pypi.org"])
  })
})
