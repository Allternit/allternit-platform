import { describe, expect, test } from "bun:test"
import fs from "fs/promises"
import os from "os"
import path from "path"
import { spawnSync } from "child_process"
import { PythonEnv } from "../../src/runtime/python/env"

describe("PythonEnv supply chain", () => {
  test("lock is hash-pinned and covers the top-level pins", () => {
    for (const pkg of PythonEnv.PACKAGES) expect(PythonEnv.LOCK).toContain(pkg)
    expect(PythonEnv.LOCK).toContain("--hash=sha256:")
    expect(PythonEnv.LOCK_HASH).toHaveLength(16)
  })

  test("uvEnv drops secrets and keeps proxy/cert vars", () => {
    const env = PythonEnv.uvEnv({
      PATH: "/bin",
      HOME: "/h",
      OPENAI_API_KEY: "sk-x",
      GITHUB_TOKEN: "t",
      HTTPS_PROXY: "http://p",
      SSL_CERT_FILE: "/c.pem",
    })
    expect(env.OPENAI_API_KEY).toBeUndefined()
    expect(env.GITHUB_TOKEN).toBeUndefined()
    expect(env.HTTPS_PROXY).toBe("http://p")
    expect(env.SSL_CERT_FILE).toBe("/c.pem")
    expect(env.PATH).toBe("/bin")
  })

  test("tampered hash is rejected by uv", async () => {
    const uv = PythonEnv.resolveUv()
    if (!uv) return
    const dir = await fs.mkdtemp(path.join(os.tmpdir(), "pyenv-tamper-"))
    try {
      // Corrupt every hash so whichever wheel uv picks fails verification.
      const bad = PythonEnv.LOCK.replace(/--hash=sha256:(.)/g, (_m, c) => `--hash=sha256:${c === "0" ? "1" : "0"}`)
      const file = path.join(dir, "bad.lock")
      await fs.writeFile(file, bad)
      const r = spawnSync(
        uv,
        ["pip", "install", "--python-version", "3.12", "--require-hashes", "--only-binary", ":all:", "--no-config", "--target", path.join(dir, "t"), "-r", file],
        { env: PythonEnv.uvEnv(), encoding: "utf8" },
      )
      expect(r.status).not.toBe(0)
      expect(`${r.stderr}`).toMatch(/hash/i)
    } finally {
      await fs.rm(dir, { recursive: true, force: true })
    }
  }, 120_000)
})
