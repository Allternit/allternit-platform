import { afterEach, describe, expect, test } from "bun:test"
import { spawnSync } from "node:child_process"
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { holdsSecret, secretByPath, workspaceListing } from "@/runtime/teleport/bundle"
import { collectProject } from "@/runtime/teleport/teleport"

// Built at run time so the repo's secret scanner does not flag this file.
const FAKE_GH_TOKEN = "ghp_" + "Z9".repeat(18)
const FAKE_PEM = "-----BEGIN OPENSSH " + "PRIVATE KEY-----\nAAAA\n"

const dirs: string[] = []
afterEach(() => {
  for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true })
})

function git(cwd: string, ...args: string[]) {
  const r = spawnSync("git", ["-C", cwd, "-c", "user.email=t@example.com", "-c", "user.name=t", "-c", "commit.gpgsign=false", ...args], {
    encoding: "utf8",
  })
  if (r.status !== 0) throw new Error(`git ${args.join(" ")}: ${r.stderr}`)
  return r.stdout.trim()
}

function put(root: string, rel: string, text: string) {
  mkdirSync(dirname(join(root, rel)), { recursive: true })
  writeFileSync(join(root, rel), text)
}

/** A project with a committed base, then edits and new files, some of them secrets. */
function project() {
  const cwd = realpathSync(mkdtempSync(join(tmpdir(), "teleport-bundle-")))
  dirs.push(cwd)
  git(cwd, "init", "-q")
  put(cwd, "src/app.ts", "export const a = 1\n")
  put(cwd, ".env", "API_URL=https://example.com\n")
  put(cwd, "config/settings.json", '{"theme":"dark"}\n')
  git(cwd, "add", "-A")
  git(cwd, "commit", "-q", "-m", "base")
  const base = git(cwd, "rev-parse", "HEAD")
  // Tracked edits: one plain, one to a tracked .env, one that pastes a token into a config file.
  put(cwd, "src/app.ts", "export const a = 2\n")
  put(cwd, ".env", `API_URL=https://example.com\nGITHUB_TOKEN=${FAKE_GH_TOKEN}\n`)
  put(cwd, "config/settings.json", `{"theme":"dark","token":"${FAKE_GH_TOKEN}"}\n`)
  // Untracked files.
  put(cwd, "notes.md", "# notes\n")
  put(cwd, "src/new.ts", "export const b = 3\n")
  put(cwd, ".env.local", "SECRET=1\n")
  put(cwd, "services/api/.ENV", "SECRET=1\n")
  put(cwd, "secrets/token.json", '{"t":1}\n')
  put(cwd, "deploy/server.pem", "pem\n")
  put(cwd, ".aws/credentials", "[default]\n")
  put(cwd, "keys/id_ed25519", FAKE_PEM)
  put(cwd, "scripts/setup.sh", `export GH=${FAKE_GH_TOKEN}\n`)
  return { cwd, base }
}

const SECRET_LOOKING = /(^|\/)\.env|token|\.pem$|id_ed25519|(^|\/)\.aws\/|setup\.sh|settings\.json/i

describe("portable workspace bundle", () => {
  test("listing has no .env, key or token file, and says what it left out", () => {
    const { cwd, base } = project()
    const listing = workspaceListing(cwd, base)

    expect(listing.changed).toEqual(["src/app.ts"])
    expect([...listing.untracked].sort()).toEqual(["notes.md", "src/new.ts"])
    for (const p of [...listing.changed, ...listing.untracked]) expect(p).not.toMatch(SECRET_LOOKING)

    const left = Object.fromEntries(listing.leftOut.map((f) => [f.path, f.reason]))
    expect(Object.keys(left).sort()).toEqual(
      [
        ".aws/credentials",
        ".env",
        ".env.local",
        "config/settings.json",
        "deploy/server.pem",
        "keys/id_ed25519",
        "scripts/setup.sh",
        "secrets/token.json",
        "services/api/.ENV",
      ].sort(),
    )
    expect(left[".aws/credentials"]).toBe("inside .aws")
    expect(left["config/settings.json"]).toBe("holds a key or token")
  })

  test("the bytes that travel hold no secret", () => {
    const { cwd, base } = project()
    const out = collectProject(cwd, base)

    const patch = Buffer.from(out.patch!).toString("utf8")
    expect(patch).toContain("src/app.ts")
    expect(patch).not.toContain(".env")
    expect(patch).not.toContain("settings.json")
    expect(patch).not.toContain(FAKE_GH_TOKEN)

    const names = spawnSync("tar", ["-tf", "-"], { input: Buffer.from(out.untracked!) }).stdout.toString("utf8").split("\n").filter(Boolean)
    expect(names.sort()).toEqual(["notes.md", "src/new.ts"])
    expect(Buffer.from(out.untracked!).toString("latin1")).not.toContain(FAKE_GH_TOKEN)
    expect(out.leftOut?.length).toBe(9)
  })

  test("path rules match whole parts, not prefixes", () => {
    expect(secretByPath(".sshx/notes.md")).toBeUndefined()
    expect(secretByPath("env.ts")).toBeUndefined()
    expect(secretByPath("docs/environment.md")).toBeUndefined()
    expect(secretByPath("home/.ssh/config")).toBe("inside .ssh")
    expect(secretByPath(".docker/config.json")).toBe("inside .docker/config.json")
    expect(secretByPath("app/.docker/Dockerfile")).toBeUndefined()
    expect(holdsSecret("const url = 'https://example.com'")).toBe(false)
    expect(holdsSecret(`token = "${FAKE_GH_TOKEN}"`)).toBe(true)
  })
})
