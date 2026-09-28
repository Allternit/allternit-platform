// E2E test for the mesh join precedence in src/runtime/server/mesh.ts: the
// mesh-node tsnet sidecar (Allternit headscale) is the primary path; a system
// tailscaled (the user's personal tailnet) is only a fallback, and userspace
// tailscaled is the last resort. Sandboxed: XDG dirs under a tmp home, fake
// `tailscale`/`tailscaled`/`mesh-node` shell scripts that record their argv
// to a file (same recorder pattern as instance-registration.test.ts's fake
// registry), and a scrubbed PATH so the real tailscale can never leak in.
//
// Flag.* and GlobalPaths are read at module load, so all env is set up in
// beforeAll before the dynamic import of mesh.ts. Per-test behavior of the
// fakes is driven by FAKE_* env vars the scripts read at exec time; the
// mesh-node binary is "missing" by removing the file GIZZI_MESH_NODE_BIN
// points at (existsSync is checked live).
import { afterAll, afterEach, beforeAll, describe, expect, test } from "bun:test"
import * as fs from "fs/promises"
import os from "os"
import path from "path"

const TAILSCALE_FAKE = `#!/bin/sh
echo "tailscale $*" >> "$MESH_TEST_RECORD"
case "$1" in
  status) exit "\${FAKE_TS_STATUS_CODE:-1}" ;;
  ip) [ -n "$FAKE_TS_IP" ] && echo "$FAKE_TS_IP"; exit 0 ;;
  up) exit "\${FAKE_TS_UP_CODE:-0}" ;;
  down) exit 0 ;;
esac
exit 0
`

const TAILSCALED_FAKE = `#!/bin/sh
echo "tailscaled $*" >> "$MESH_TEST_RECORD"
sleep 60
`

const MESH_NODE_FAKE = `#!/bin/sh
echo "mesh-node $*" >> "$MESH_TEST_RECORD"
if [ "$FAKE_NODE_FAIL" = "1" ]; then
  echo "MESH_ERROR reason=boom" >&2
  exit 1
fi
echo "MESH_READY ip=\${FAKE_NODE_IP:-100.64.0.7}"
exec tail -f /dev/null
`

let home: string
let fakebin: string
let nodeBin: string
let nodeFake: string
let record: string
let originalPath: string | undefined
const previousFlag: Record<string, string | undefined> = {}
let Log: typeof import("../../src/shared/util/log").Log
let Mesh: typeof import("../../src/runtime/server/mesh").Mesh

async function writeExec(file: string, contents: string) {
  await fs.writeFile(file, contents)
  await fs.chmod(file, 0o755)
}

async function recorded(): Promise<string> {
  return fs.readFile(record, "utf8").catch(() => "")
}

function wait(ms: number) {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

beforeAll(async () => {
  home = await fs.mkdtemp(path.join(os.tmpdir(), "gizzi-mesh-test-"))
  process.env.XDG_DATA_HOME = path.join(home, "share")
  process.env.XDG_CONFIG_HOME = path.join(home, "config")
  process.env.XDG_CACHE_HOME = path.join(home, "cache")
  process.env.XDG_STATE_HOME = path.join(home, "state")

  fakebin = path.join(home, "fakebin")
  await fs.mkdir(fakebin, { recursive: true })
  await writeExec(path.join(fakebin, "tailscale"), TAILSCALE_FAKE)
  await writeExec(path.join(fakebin, "tailscaled"), TAILSCALED_FAKE)

  // Scrub PATH so the real tailscale/tailscaled/mesh-node can never be found
  // through the PATH fallback in findBinary.
  originalPath = process.env.PATH
  process.env.PATH = `${fakebin}:/usr/bin:/bin`

  record = path.join(home, "record")
  process.env.MESH_TEST_RECORD = record

  // Binary overrides are snapshotted by Flag at module load; mesh-node's
  // target file is created/removed per test to simulate present/missing.
  nodeBin = path.join(home, "bin", "mesh-node")
  await fs.mkdir(path.dirname(nodeBin), { recursive: true })
  // The fake is written once, here; tests only link/unlink nodeBin. Writing
  // an executable right before spawning it races on Linux (ETXTBSY when a
  // concurrent fork still holds the write fd), which made mesh-node fail to
  // start and test (c) flaky in CI.
  nodeFake = path.join(home, "mesh-node-fake")
  await writeExec(nodeFake, MESH_NODE_FAKE)
  process.env.GIZZI_MESH_NODE_BIN = nodeBin
  process.env.GIZZI_TAILSCALE_BIN = path.join(fakebin, "tailscale")
  process.env.GIZZI_TAILSCALED_BIN = path.join(fakebin, "tailscaled")

  // Log.init writes under Global.Path.log; create it so the warnings are
  // actually captured to the log file. Both imports are dynamic: GlobalPaths
  // and Flag snapshot the env at module load, so they must happen after the
  // XDG/override setup above.
  await fs.mkdir(path.join(process.env.XDG_DATA_HOME, "gizzi-code", "log"), { recursive: true })
  Log = (await import("../../src/shared/util/log")).Log
  await Log.init({ print: false })

  // Flag consts are frozen at first module import; in the full smoke suite
  // other files import Flag before this beforeAll runs, so the env overrides
  // above would be ignored (Bun.which also only sees the process-start PATH,
  // so the PATH scrub cannot make the fakes discoverable either). The Flag
  // namespace is runtime-mutable — point the binary overrides at the fakes
  // directly and restore the previous values in afterAll.
  const mutableFlag = (await import("../../src/runtime/context/flag/flag")).Flag as unknown as Record<string, string | undefined>
  for (const key of ["GIZZI_MESH_NODE_BIN", "GIZZI_TAILSCALE_BIN", "GIZZI_TAILSCALED_BIN"]) {
    previousFlag[key] = mutableFlag[key]
    mutableFlag[key] = process.env[key]
  }

  Mesh = (await import("../../src/runtime/server/mesh")).Mesh
})

afterEach(async () => {
  await Mesh.stop()
  await fs.rm(nodeBin, { force: true })
  for (const key of ["FAKE_TS_STATUS_CODE", "FAKE_TS_IP", "FAKE_TS_UP_CODE", "FAKE_NODE_FAIL", "FAKE_NODE_IP"]) {
    delete process.env[key]
  }
})

afterAll(async () => {
  process.env.PATH = originalPath
  delete process.env.MESH_TEST_RECORD
  delete process.env.GIZZI_MESH_NODE_BIN
  delete process.env.GIZZI_TAILSCALE_BIN
  delete process.env.GIZZI_TAILSCALED_BIN
  const mutableFlag = (await import("../../src/runtime/context/flag/flag")).Flag as unknown as Record<string, string | undefined>
  for (const key of ["GIZZI_MESH_NODE_BIN", "GIZZI_TAILSCALE_BIN", "GIZZI_TAILSCALED_BIN"]) {
    mutableFlag[key] = previousFlag[key]
  }
  await fs.rm(home, { recursive: true, force: true })
})

describe("Mesh join precedence", () => {
  test("(a) sidecar wins over a reachable system tailscaled; tailscale CLI is never invoked", async () => {
    await fs.symlink(nodeFake, nodeBin)
    await Bun.write(record, "")
    // A system tailscaled logged into a FOREIGN (personal) tailnet.
    process.env.FAKE_TS_STATUS_CODE = "0"
    process.env.FAKE_TS_IP = "100.99.0.5"

    const url = await Mesh.start(4096, { authKey: "tskey-test" })

    expect(url).toBe("http://100.64.0.7:4096")
    const calls = await recorded()
    expect(calls).toContain("mesh-node ")
    expect(calls).toContain("--auth-key tskey-test")
    expect(calls).not.toContain("tailscale")
  })

  test("(b) sidecar missing + system tailscaled present -> attach fallback (passive reuse)", async () => {
    // No mesh-node file on disk; a foreign-tailnet system daemon answers.
    await Bun.write(record, "")
    process.env.FAKE_TS_STATUS_CODE = "0"
    process.env.FAKE_TS_IP = "100.99.0.5"

    const url = await Mesh.start(4096, { authKey: "tskey-test" })

    expect(url).toBe("http://100.99.0.5:4096")
    const calls = await recorded()
    expect(calls).not.toContain("mesh-node")
    expect(calls).toContain("tailscale status")
    expect(calls).toContain("tailscale ip -4")
    // Passive reuse: never `up` on someone else's login.
    expect(calls).not.toContain("tailscale up")
  })

  test("DEBUG lifeline probe", async () => {
    const { spawnOwnedChild } = await import("../../src/runtime/util/parent-lifeline")
    for (const cmd of [["/bin/sh", ["-c", "sleep 0.3; echo alive"]], [nodeFake, []]] as const) {
      const started = Date.now()
      const proc = spawnOwnedChild(cmd[0], [...cmd[1]], { stdio: ["ignore", "pipe", "pipe"] })
      let out = ""
      proc.stdout?.on("data", (c: Buffer) => (out += c.toString()))
      proc.stderr?.on("data", (c: Buffer) => (out += "ERR:" + c.toString()))
      const killer = setTimeout(() => proc.kill("SIGTERM"), 800)
      const res = await new Promise<string>((r) => proc.once("exit", (code, sig) => r(`code=${code} sig=${sig}`)))
      clearTimeout(killer)
      console.log(`MESHDEBUG probe ${cmd[0]} ${res} after ${Date.now() - started}ms out=${JSON.stringify(out)}`)
      if (cmd[0] === nodeFake) break
    }
  })

  test("(c) sidecar present but join fails -> attach fallback with a warning", async () => {
    await fs.symlink(nodeFake, nodeBin)
    await Bun.write(record, "")
    process.env.FAKE_NODE_FAIL = "1"
    process.env.FAKE_TS_STATUS_CODE = "0"
    process.env.FAKE_TS_IP = "100.99.0.5"

    const url = await Mesh.start(4096, { authKey: "tskey-test" })

    expect(url).toBe("http://100.99.0.5:4096")
    const calls = await recorded()
    await wait(100)
    console.log("MESHDEBUG record:\n" + calls)
    console.log("MESHDEBUG log:\n" + (await fs.readFile(Log.file(), "utf8")).split("\n").filter((l) => /mesh|sidecar|tailscale/i.test(l)).slice(-15).join("\n"))
    // Sidecar was attempted first, then the system tailscaled took over.
    expect(calls).toContain("mesh-node ")
    expect(calls).toContain("tailscale status")
    await wait(100)
    const logfile = await fs.readFile(Log.file(), "utf8")
    expect(logfile).toContain("mesh-node sidecar failed; falling back to a system tailscaled")
  })

  test("(d) no auth key -> mesh skipped with a hint; no binary is invoked", async () => {
    await fs.symlink(nodeFake, nodeBin)
    await Bun.write(record, "")
    process.env.FAKE_TS_STATUS_CODE = "0"
    process.env.FAKE_TS_IP = "100.99.0.5"

    const url = await Mesh.start(4096)

    expect(url).toBeUndefined()
    expect(await recorded()).toBe("")
    await wait(100)
    const logfile = await fs.readFile(Log.file(), "utf8")
    expect(logfile).toContain("no mesh auth key configured; skipping the mesh join")
    expect(logfile).toContain("gizzi pair")
  })
})
