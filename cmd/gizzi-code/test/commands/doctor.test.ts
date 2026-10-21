import { describe, expect, test } from "bun:test"
import path from "path"
import { tmpdir } from "../fixture/fixture"
import {
  checkCredentialSecurity,
  checkCronDaemon,
  checkGatewayReachability,
  checkProjectInstructions,
  checkVoiceEngine,
} from "../../src/cli/commands/doctorChecks"
import { readAuthProfiles } from "../../src/runtime/context/config/auth-profiles"

describe("checkProjectInstructions", () => {
  test("passes when GIZZI.md is present", async () => {
    await using tmp = await tmpdir()
    await Bun.write(path.join(tmp.path, "GIZZI.md"), "# project instructions")
    const result = await checkProjectInstructions(tmp.path)
    expect(result.status).toBe("pass")
    expect(result.message).toContain("GIZZI.md")
  })

  test("warns when absent", async () => {
    await using tmp = await tmpdir()
    const result = await checkProjectInstructions(tmp.path)
    expect(result.status).toBe("warn")
  })

  test("warns (with legacy note) when only CLAUDE.md exists", async () => {
    await using tmp = await tmpdir()
    await Bun.write(path.join(tmp.path, "CLAUDE.md"), "# legacy")
    const result = await checkProjectInstructions(tmp.path)
    expect(result.status).toBe("warn")
    expect(result.message).toContain("CLAUDE.md")
  })

  test("honours an injected exists() for hermetic checks", async () => {
    const result = await checkProjectInstructions("/anywhere", (p) => p.endsWith("GIZZI.md"))
    expect(result.status).toBe("pass")
  })
})

describe("checkGatewayReachability", () => {
  test("reports ok on any HTTP response", async () => {
    const original = globalThis.fetch
    globalThis.fetch = (async () => new Response("{}", { status: 404 })) as unknown as typeof fetch
    try {
      const result = await checkGatewayReachability({ baseUrl: "http://127.0.0.1:9999", offline: false })
      expect(result.status).toBe("pass")
      expect(result.message).toContain("HTTP 404")
    } finally {
      globalThis.fetch = original
    }
  })

  test("reports unreachable when the fetch fails", async () => {
    const original = globalThis.fetch
    globalThis.fetch = (async () => {
      throw new Error("connect ECONNREFUSED")
    }) as unknown as typeof fetch
    try {
      const result = await checkGatewayReachability({ baseUrl: "http://127.0.0.1:9999", offline: false })
      expect(result.status).toBe("fail")
      expect(result.message).toContain("unreachable")
    } finally {
      globalThis.fetch = original
    }
  })

  test("reports offline mode without network access", async () => {
    let called = false
    const original = globalThis.fetch
    globalThis.fetch = (async () => {
      called = true
      return new Response("{}")
    }) as unknown as typeof fetch
    try {
      const result = await checkGatewayReachability({ offline: true })
      expect(result.status).toBe("pass")
      expect(result.message).toContain("offline mode")
      expect(called).toBe(false)
    } finally {
      globalThis.fetch = original
    }
  })
})

describe("checkCronDaemon", () => {
  test("passes when the daemon is running", async () => {
    const results = await checkCronDaemon({ isRunning: async () => true })
    expect(results.find((r) => r.id === "cron-daemon")?.status).toBe("pass")
  })

  test("warns when the daemon is not running", async () => {
    const results = await checkCronDaemon({ isRunning: async () => false })
    expect(results.find((r) => r.id === "cron-daemon")?.status).toBe("warn")
  })

  test("reports supervised state when a probe is wired", async () => {
    const results = await checkCronDaemon({
      isRunning: async () => true,
      supervised: async () => ({ launchdPlist: "/tmp/com.example.plist", systemdUnit: null, supported: true }),
    })
    expect(results.find((r) => r.id === "cron-autostart")?.status).toBe("pass")
    expect(results.find((r) => r.id === "cron-autostart")?.message).toContain("/tmp/com.example.plist")
  })
})

describe("checkCredentialSecurity", () => {
  test("passes when the fallback credentials file is 0600", async () => {
    await using tmp = await tmpdir()
    const credPath = path.join(tmp.path, "credentials.json")
    await Bun.write(credPath, "{}")
    await Bun.$`chmod 600 ${credPath}`
    const results = await checkCredentialSecurity({
      credentialsPath: credPath,
      configTomlPath: path.join(tmp.path, "config.toml"),
    })
    expect(results.find((r) => r.id === "credentials-permissions")?.status).toBe("pass")
  })

  test("fails when the fallback credentials file is group/world readable", async () => {
    await using tmp = await tmpdir()
    const credPath = path.join(tmp.path, "credentials.json")
    await Bun.write(credPath, "{}")
    await Bun.$`chmod 644 ${credPath}`
    const results = await checkCredentialSecurity({
      credentialsPath: credPath,
      configTomlPath: path.join(tmp.path, "config.toml"),
    })
    expect(results.find((r) => r.id === "credentials-permissions")?.status).toBe("fail")
  })

  test("passes permissions when no fallback file exists", async () => {
    await using tmp = await tmpdir()
    const results = await checkCredentialSecurity({
      credentialsPath: path.join(tmp.path, "missing.json"),
      configTomlPath: path.join(tmp.path, "config.toml"),
    })
    expect(results.find((r) => r.id === "credentials-permissions")?.status).toBe("pass")
  })

  test("fails when config.toml holds an inline api_key", async () => {
    await using tmp = await tmpdir()
    const configPath = path.join(tmp.path, "config.toml")
    await Bun.write(
      configPath,
      [
        "[auth]",
        'active_profile = "default"',
        "",
        "[auth.profiles.default]",
        'provider = "anthropic"',
        'api_key = "sk-test-inline"',
      ].join("\n"),
    )
    const auth = await readAuthProfiles(configPath)
    expect(auth.profiles.default?.api_key).toBe("sk-test-inline")
    const results = await checkCredentialSecurity({
      credentialsPath: path.join(tmp.path, "credentials.json"),
      configTomlPath: configPath,
    })
    expect(results.find((r) => r.id === "config-inline-api-key")?.status).toBe("fail")
    expect(results.find((r) => r.id === "config-inline-api-key")?.message).toContain("default")
  })

  test("passes when config.toml has no inline keys", async () => {
    await using tmp = await tmpdir()
    const configPath = path.join(tmp.path, "config.toml")
    await Bun.write(configPath, ['[auth.profiles.default]', 'provider = "anthropic"', 'api_key_env = "ANTHROPIC_API_KEY"'].join("\n"))
    const results = await checkCredentialSecurity({
      credentialsPath: path.join(tmp.path, "credentials.json"),
      configTomlPath: configPath,
    })
    expect(results.find((r) => r.id === "config-inline-api-key")?.status).toBe("pass")
  })
})

describe("checkVoiceEngine", () => {
  test("passes when the sidecar health endpoint is ok", async () => {
    const original = globalThis.fetch
    globalThis.fetch = (async () =>
      new Response(
        JSON.stringify({ engine: "sherpa-onnx", packs: [{ name: "small", state: "ready" }] }),
        { status: 200, headers: { "content-type": "application/json" } },
      )) as unknown as typeof fetch
    try {
      const result = await checkVoiceEngine()
      expect(result.status).toBe("pass")
      expect(result.message).toContain("sherpa-onnx")
    } finally {
      globalThis.fetch = original
    }
  })

  test("warns when a voice pack failed to download", async () => {
    const original = globalThis.fetch
    globalThis.fetch = (async () =>
      new Response(
        JSON.stringify({
          engine: "sherpa-onnx",
          packs: [{ name: "small", state: "error", error: "sha256 mismatch" }],
        }),
        { status: 200, headers: { "content-type": "application/json" } },
      )) as unknown as typeof fetch
    try {
      const result = await checkVoiceEngine()
      expect(result.status).toBe("warn")
      expect(result.message).toContain("sha256 mismatch")
    } finally {
      globalThis.fetch = original
    }
  })

  test("warns when the sidecar is down", async () => {
    const original = globalThis.fetch
    globalThis.fetch = (async () => {
      throw new Error("connection refused")
    }) as unknown as typeof fetch
    try {
      const result = await checkVoiceEngine()
      expect(result.status).toBe("warn")
      expect(result.section).toBe("Voice")
    } finally {
      globalThis.fetch = original
    }
  })
})
