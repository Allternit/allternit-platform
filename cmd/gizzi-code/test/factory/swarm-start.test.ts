import { afterEach, beforeEach, describe, expect, test } from "bun:test"
import { spawnSync } from "node:child_process"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { startSwarm } from "../../src/cli/factory/swarm"

// A fake `allternit-factory`. Every call saves the team.yaml it would read and
// its args under $FAKE_LOG. The first `agents up` fails w1; later ones succeed
// for the newest bot, unless the team file still lists more than one bot (the
// real engine refuses that: two bots would share one AGENTS.md).
const FAKE_ENGINE = `#!/bin/sh
n=$(ls "$FAKE_LOG" | grep -c '\\.args$')
n=$((n+1))
echo "$@" > "$FAKE_LOG/call-$n.args"
root=""; prev=""
for a in "$@"; do [ "$prev" = "--root" ] && root="$a"; prev="$a"; done
if [ "$1" = "agents" ] && [ "$2" = "up" ]; then
  team="$3"
  yaml="$root/.allternit/teams/$team/team.yaml"
  cp "$yaml" "$FAKE_LOG/call-$n.yaml"
  bots=$(grep -c 'bot: ' "$yaml")
  last=$(grep -o 'bot: w[0-9]*' "$yaml" | tail -1 | sed 's/bot: //')
  if [ ! -f "$FAKE_LOG/up-seen" ]; then
    touch "$FAKE_LOG/up-seen"
    printf '{"results":[{"outcome":"failed","fact":"pane would not open","step":{"agent":"%s@%s","action":"spawn"}}]}\\n' "$last" "$team"
  elif [ "$bots" -gt 1 ]; then
    printf '{"results":[{"outcome":"failed","fact":"w1 and w2 would share AGENTS.md","step":{"agent":"%s@%s","action":"spawn"}}]}\\n' "$last" "$team"
  else
    printf '{"results":[{"outcome":"ok","step":{"agent":"%s@%s","action":"spawn"}}]}\\n' "$last" "$team"
  fi
elif [ "$1" = "agents" ] && [ "$2" = "ps" ]; then
  printf '{"agents":[{"id":"local:w1-x","address":"w1@x","state":"idle"},{"id":"local:w2-x","address":"w2@x","state":"idle"}]}\\n'
elif [ "$1" = "orchestration" ]; then
  printf '{"state":"verified"}\\n'
else
  printf '{}\\n'
fi
`

let tmp = ""
let prevBin: string | undefined
let prevHome: string | undefined

function sh(cwd: string, args: string[]) {
  const r = spawnSync("git", args, { cwd, encoding: "utf8" })
  if (r.status !== 0) throw new Error(`git ${args.join(" ")}: ${r.stderr}`)
}

beforeEach(() => {
  tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "swarm-start-")))
  fs.mkdirSync(path.join(tmp, "log"))
  const bin = path.join(tmp, "allternit-factory-fake")
  fs.writeFileSync(bin, FAKE_ENGINE, { mode: 0o755 })
  prevBin = process.env.ALLTERNIT_FACTORY_BIN
  prevHome = process.env.ALLTERNIT_FACTORY_HOME
  process.env.ALLTERNIT_FACTORY_BIN = bin
  process.env.ALLTERNIT_FACTORY_HOME = path.join(tmp, "home")
  process.env.FAKE_LOG = path.join(tmp, "log")
})

afterEach(() => {
  if (prevBin === undefined) delete process.env.ALLTERNIT_FACTORY_BIN
  else process.env.ALLTERNIT_FACTORY_BIN = prevBin
  if (prevHome === undefined) delete process.env.ALLTERNIT_FACTORY_HOME
  else process.env.ALLTERNIT_FACTORY_HOME = prevHome
  delete process.env.FAKE_LOG
  fs.rmSync(tmp, { recursive: true, force: true })
})

describe("startSwarm", () => {
  test("a worker whose `up` failed is taken out of the team file, so it can't take down the next worker", async () => {
    const repo = path.join(tmp, "repo")
    fs.mkdirSync(repo)
    sh(repo, ["init", "-q"])
    sh(repo, ["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "init"])

    const { record } = await startSwarm({
      task: "",
      tasks: ["a", "b"],
      workers: 2,
      brain: "codex-cli/x",
      cwd: repo,
      log: () => {},
      readyTimeoutMs: 200,
    })

    const ups = fs
      .readdirSync(path.join(tmp, "log"))
      .filter((f) => f.endsWith(".args") && fs.readFileSync(path.join(tmp, "log", f), "utf8").startsWith("agents up"))
      .sort()
    expect(ups.length).toBe(2)
    const yamlOf = (argsFile: string) => fs.readFileSync(path.join(tmp, "log", argsFile.replace(".args", ".yaml")), "utf8")
    expect(yamlOf(ups[0]!)).toContain("bot: w1")
    const second = yamlOf(ups[1]!)
    expect(second).not.toContain("bot: w1")
    expect(second).toContain("bot: w2")

    const w1 = record.workers.find((w) => w.bot === "w1")!
    const w2 = record.workers.find((w) => w.bot === "w2")!
    expect(w1.phase).toBe("failed")
    expect(w1.note).toBe("pane would not open")
    expect(w2.phase).not.toBe("failed")
    expect(w2.note ?? "").not.toContain("AGENTS.md")

    // clean up the worktrees the swarm made
    for (const w of record.workers) spawnSync("git", ["worktree", "remove", "--force", w.workdir], { cwd: repo })
  })
})
