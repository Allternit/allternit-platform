import { describe, expect, test } from "bun:test"
import os from "os"
import path from "path"
import fs from "fs/promises"
import {
  RunCodeTool,
  SANDBOX_CANNOT_START,
  readAttachment,
  sandboxStartFailure,
} from "../../src/runtime/tools/builtins/run_code"
import { Sandbox, type SandboxPolicy } from "../../src/runtime/integrations/shell/sandbox"

// The first run sets up the real uv venv (network). Reuse one env dir across
// local runs; CI without network sets GIZZI_SKIP_RUN_CODE_E2E=1.
process.env.GIZZI_PYTHON_ENV_DIR ??= path.join(os.tmpdir(), "gizzi-run-code-test-python")
const skip = process.env.GIZZI_SKIP_RUN_CODE_E2E === "1" || process.platform === "win32"

const CSV = "month,revenue\nJan,10\nFeb,30\nMar,20\n"
const exists = (p: string) => fs.access(p).then(
  () => true,
  () => false,
)

/**
 * The default (non-strict) Seatbelt profile and bwrap args, captured from the
 * code before `strict` existed ($HOME / os.tmpdir() replaced by tokens). Bash,
 * fabric workers and the agentic runner use this policy: it must not drift.
 */
const GOLDEN: Record<string, string | string[]> = {
 "sb_off": "(version 1)\n(deny default)\n\n; ── Process execution ───────────────────────────────────────\n(allow process-exec*)\n(allow process-fork)\n(allow process-info*)\n(allow signal (target self))\n(allow signal (target children))\n\n; ── IPC / Mach ──────────────────────────────────────────────\n(allow ipc-posix*)\n(allow mach-lookup)\n(allow mach-priv-host-port)\n(allow mach-task-name)\n\n; ── System info ─────────────────────────────────────────────\n(allow sysctl-read)\n(allow system-socket)\n\n; ── File reads: entire filesystem ───────────────────────────\n(allow file-read*)\n(allow file-test-existence)\n\n; ── File writes: declared paths only ────────────────────────\n(allow file-write*\n  (subpath \"/work/dir\")\n  (subpath \"${TMP}\")\n  (subpath \"/var/tmp\")\n  (subpath \"/private/tmp\")\n  (subpath \"/private/var/folders\")\n  (subpath \"${HOME}/.cache\")\n  (subpath \"${HOME}/.npm\")\n  (subpath \"${HOME}/.cargo\")\n  (subpath \"${HOME}/.pnpm-store\")\n  (subpath \"${HOME}/.bun\")\n  (subpath \"/extra/w\")\n)\n\n; ── Network ─────────────────────────────────────────────────\n; network blocked\n\n; ── Devices / IOKit ─────────────────────────────────────────\n(allow file-ioctl)\n(allow iokit-open)\n",
 "bw_off": [
  "--ro-bind-try",
  "/usr",
  "/usr",
  "--ro-bind-try",
  "/etc",
  "/etc",
  "--ro-bind-try",
  "/opt",
  "/opt",
  "--ro-bind-try",
  "/bin",
  "/bin",
  "--ro-bind-try",
  "/sbin",
  "/sbin",
  "--ro-bind-try",
  "/lib",
  "/lib",
  "--ro-bind-try",
  "/lib64",
  "/lib64",
  "--ro-bind-try",
  "/lib32",
  "/lib32",
  "--ro-bind-try",
  "/extra/r",
  "/extra/r",
  "--bind-try",
  "/work/dir",
  "/work/dir",
  "--bind-try",
  "${TMP}",
  "${TMP}",
  "--bind-try",
  "/var/tmp",
  "/var/tmp",
  "--bind-try",
  "/private/tmp",
  "/private/tmp",
  "--bind-try",
  "/private/var/folders",
  "/private/var/folders",
  "--bind-try",
  "${HOME}/.cache",
  "${HOME}/.cache",
  "--bind-try",
  "${HOME}/.npm",
  "${HOME}/.npm",
  "--bind-try",
  "${HOME}/.cargo",
  "${HOME}/.cargo",
  "--bind-try",
  "${HOME}/.pnpm-store",
  "${HOME}/.pnpm-store",
  "--bind-try",
  "${HOME}/.bun",
  "${HOME}/.bun",
  "--bind-try",
  "/extra/w",
  "/extra/w",
  "--proc",
  "/proc",
  "--dev",
  "/dev",
  "--tmpfs",
  "/run",
  "--chdir",
  "/work/dir",
  "--die-with-parent",
  "--unshare-net",
  "--",
  "/bin/sh",
  "-c",
  "echo hi"
 ],
 "sb_on": "(version 1)\n(deny default)\n\n; ── Process execution ───────────────────────────────────────\n(allow process-exec*)\n(allow process-fork)\n(allow process-info*)\n(allow signal (target self))\n(allow signal (target children))\n\n; ── IPC / Mach ──────────────────────────────────────────────\n(allow ipc-posix*)\n(allow mach-lookup)\n(allow mach-priv-host-port)\n(allow mach-task-name)\n\n; ── System info ─────────────────────────────────────────────\n(allow sysctl-read)\n(allow system-socket)\n\n; ── File reads: entire filesystem ───────────────────────────\n(allow file-read*)\n(allow file-test-existence)\n\n; ── File writes: declared paths only ────────────────────────\n(allow file-write*\n  (subpath \"/work/dir\")\n  (subpath \"${TMP}\")\n  (subpath \"/var/tmp\")\n  (subpath \"/private/tmp\")\n  (subpath \"/private/var/folders\")\n  (subpath \"${HOME}/.cache\")\n  (subpath \"${HOME}/.npm\")\n  (subpath \"${HOME}/.cargo\")\n  (subpath \"${HOME}/.pnpm-store\")\n  (subpath \"${HOME}/.bun\")\n)\n\n; ── Network ─────────────────────────────────────────────────\n(allow network*)\n\n; ── Devices / IOKit ─────────────────────────────────────────\n(allow file-ioctl)\n(allow iokit-open)\n",
 "bw_on": [
  "--ro-bind-try",
  "/usr",
  "/usr",
  "--ro-bind-try",
  "/etc",
  "/etc",
  "--ro-bind-try",
  "/opt",
  "/opt",
  "--ro-bind-try",
  "/bin",
  "/bin",
  "--ro-bind-try",
  "/sbin",
  "/sbin",
  "--ro-bind-try",
  "/lib",
  "/lib",
  "--ro-bind-try",
  "/lib64",
  "/lib64",
  "--ro-bind-try",
  "/lib32",
  "/lib32",
  "--bind-try",
  "/work/dir",
  "/work/dir",
  "--bind-try",
  "${TMP}",
  "${TMP}",
  "--bind-try",
  "/var/tmp",
  "/var/tmp",
  "--bind-try",
  "/private/tmp",
  "/private/tmp",
  "--bind-try",
  "/private/var/folders",
  "/private/var/folders",
  "--bind-try",
  "${HOME}/.cache",
  "${HOME}/.cache",
  "--bind-try",
  "${HOME}/.npm",
  "${HOME}/.npm",
  "--bind-try",
  "${HOME}/.cargo",
  "${HOME}/.cargo",
  "--bind-try",
  "${HOME}/.pnpm-store",
  "${HOME}/.pnpm-store",
  "--bind-try",
  "${HOME}/.bun",
  "${HOME}/.bun",
  "--proc",
  "/proc",
  "--dev",
  "/dev",
  "--tmpfs",
  "/run",
  "--chdir",
  "/work/dir",
  "--die-with-parent",
  "--",
  "/bin/sh",
  "-c",
  "echo hi"
 ],
 "sb_tcp": "(version 1)\n(deny default)\n\n; ── Process execution ───────────────────────────────────────\n(allow process-exec*)\n(allow process-fork)\n(allow process-info*)\n(allow signal (target self))\n(allow signal (target children))\n\n; ── IPC / Mach ──────────────────────────────────────────────\n(allow ipc-posix*)\n(allow mach-lookup)\n(allow mach-priv-host-port)\n(allow mach-task-name)\n\n; ── System info ─────────────────────────────────────────────\n(allow sysctl-read)\n(allow system-socket)\n\n; ── File reads: entire filesystem ───────────────────────────\n(allow file-read*)\n(allow file-test-existence)\n\n; ── File writes: declared paths only ────────────────────────\n(allow file-write*\n  (subpath \"/work/dir\")\n  (subpath \"${TMP}\")\n  (subpath \"/var/tmp\")\n  (subpath \"/private/tmp\")\n  (subpath \"/private/var/folders\")\n  (subpath \"${HOME}/.cache\")\n  (subpath \"${HOME}/.npm\")\n  (subpath \"${HOME}/.cargo\")\n  (subpath \"${HOME}/.pnpm-store\")\n  (subpath \"${HOME}/.bun\")\n)\n\n; ── Network ─────────────────────────────────────────────────\n(allow network-outbound (remote ip \"localhost:4321\"))\n\n; ── Devices / IOKit ─────────────────────────────────────────\n(allow file-ioctl)\n(allow iokit-open)\n"
}

const normalize = (v: unknown) =>
  JSON.parse(JSON.stringify(v).split(os.homedir()).join("${HOME}").split(os.tmpdir()).join("${TMP}"))

describe("sandbox policy", () => {
  const cwd = "/work/dir"
  const off: SandboxPolicy = { allowWritePaths: ["/extra/w"], allowNetwork: false, allowReadPaths: ["/extra/r"] }
  const on: SandboxPolicy = { allowWritePaths: [], allowNetwork: true }

  test("non-strict Seatbelt profiles are byte-identical to before", () => {
    expect(normalize(Sandbox.seatbeltProfile(cwd, off))).toBe(GOLDEN.sb_off as string)
    expect(normalize(Sandbox.seatbeltProfile(cwd, on))).toBe(GOLDEN.sb_on as string)
    expect(
      normalize(Sandbox.seatbeltProfile(cwd, { ...on, allowedDomains: ["x.org"] }, { mode: "tcp", port: 4321 } as never)),
    ).toBe(GOLDEN.sb_tcp as string)
  })

  test("non-strict bwrap args are identical to before", () => {
    expect(normalize(Sandbox.bwrapArgs("echo hi", "/bin/sh", cwd, off).args)).toEqual(GOLDEN.bw_off)
    expect(normalize(Sandbox.bwrapArgs("echo hi", "/bin/sh", cwd, on).args)).toEqual(GOLDEN.bw_on)
  })

  test("strict bwrap: private /tmp first, run folder is the only write bind, pid/ipc/net unshared", () => {
    const args = Sandbox.bwrapArgs("echo hi", "/bin/sh", cwd, {
      strict: true,
      allowWritePaths: [cwd],
      allowNetwork: true, // strict ignores this
      allowReadPaths: ["/py/env"],
    }).args
    expect(args.slice(0, 2)).toEqual(["--tmpfs", "/tmp"])
    const writeBinds = args.flatMap((a, i) => (a === "--bind-try" ? [args[i + 1]] : []))
    expect([...new Set(writeBinds)]).toEqual([cwd])
    expect(args.join(" ")).toContain("--ro-bind-try /py/env /py/env")
    for (const flag of ["--unshare-pid", "--unshare-ipc", "--new-session", "--unshare-net", "--die-with-parent"]) {
      expect(args).toContain(flag)
    }
    expect(args.join(" ")).not.toContain(".cargo")
  })

  test("strict Seatbelt: home denied, run folder re-allowed, no iokit, writes to the run folder only", () => {
    const profile = Sandbox.seatbeltProfile(cwd, { strict: true, allowWritePaths: [cwd], allowNetwork: true })
    expect(profile).toContain(`(deny file-read*\n  (subpath "${os.homedir()}")`)
    expect(profile).toContain(`(subpath "/Library/Keychains")`)
    expect(profile).not.toContain("iokit-open")
    expect(profile).not.toContain("network")
    expect(profile).not.toContain("(allow mach-lookup)")
    // The only Mach service: getpwuid. pandas + matplotlib (text labels included) need no others.
    expect(profile.match(/global-name "[^"]+"/g)).toEqual(['global-name "com.apple.system.opendirectoryd.libinfo"'])
    expect(profile).toMatch(/\(allow file-write\*\n  \(subpath "\/work\/dir"\)\n\)/)
  })
})

describe.skipIf(process.platform !== "darwin" && Sandbox.detect() === "none")("default (bash) sandbox, really run", () => {
  // The profile now goes inline (`sandbox-exec -p`) instead of a file in tmp;
  // bash's posture itself must not change: home readable, temp writable.
  test("non-strict wrap reads home and writes temp, and strict does neither", async () => {
    const tag = `${process.pid}-${Date.now()}`
    const cwd = await fs.mkdtemp(path.join(os.tmpdir(), "gizzi-wrap-"))
    const homeFile = path.join(os.homedir(), `.gizzi-wrap-probe-${tag}`)
    const tmpFile = path.join(os.tmpdir(), `gizzi-wrap-tmp-${tag}`)
    await fs.writeFile(homeFile, "HOME-OK")
    const run = async (policy: SandboxPolicy) => {
      const w = (await Sandbox.wrap({
        command: `cat "${homeFile}"; echo; echo x > "${tmpFile}" && echo TMP-WRITE-OK`,
        shell: "/bin/sh",
        cwd,
        sessionID: `wrap-test-${tag}`,
        policy,
      }))!
      const proc = Bun.spawn([w.bin, ...w.args], { cwd, stdout: "pipe", stderr: "pipe" })
      await proc.exited
      const wroteHostTmp = await exists(tmpFile)
      await fs.rm(tmpFile, { force: true })
      return { out: (await new Response(proc.stdout).text()) + (await new Response(proc.stderr).text()), wroteHostTmp }
    }
    try {
      const loose = await run({ allowWritePaths: [], allowNetwork: false })
      expect(loose.out).toContain("HOME-OK")
      expect(loose.out).toContain("TMP-WRITE-OK")
      expect(loose.wroteHostTmp).toBe(true)
      const strict = await run({ strict: true, allowWritePaths: [cwd], allowNetwork: false })
      expect(strict.out).not.toContain("HOME-OK")
      // Linux strict gives the script a private /tmp, so the write may succeed there; it never reaches the host.
      expect(strict.wroteHostTmp).toBe(false)
    } finally {
      await Sandbox.cleanupProfile(`wrap-test-${tag}`)
      await Promise.all([homeFile, tmpFile, cwd].map((f) => fs.rm(f, { recursive: true, force: true })))
    }
  })
})

describe("run_code sandbox start failure", () => {
  // No seam makes the real bwrap fail on this Mac, so the detector that turns
  // the driver's own setup error into the one-line message is tested directly.
  test("bwrap's setup error on an unprivileged container is recognized", () => {
    const line = "bwrap: Failed to make / slave: Permission denied"
    expect(sandboxStartFailure("/usr/bin/bwrap", 1, "", `${line}\n`)).toBe(line)
    expect(sandboxStartFailure("sandbox-exec", 71, "", "sandbox-exec: sandbox_apply: Operation not permitted\n")).toContain(
      "sandbox_apply",
    )
  })
  test("a script's own failure is not mistaken for it", () => {
    expect(sandboxStartFailure("bwrap", 1, "", "Traceback (most recent call last):\n")).toBeUndefined()
    expect(sandboxStartFailure("bwrap", 1, "partial output", "bwrap: x\n")).toBeUndefined()
    expect(sandboxStartFailure("bwrap", 0, "", "bwrap: x\n")).toBeUndefined()
  })
  test("the message is one line for the person", () => {
    expect(SANDBOX_CANNOT_START).toBe("Running code isn't available on this computer yet: the sandbox can't start.")
  })
})

describe("run_code attachments", () => {
  const signal = AbortSignal.any([])
  test("data: URLs are read", async () => {
    expect((await readAttachment("data:text/plain;base64,aGk=", signal)).toString()).toBe("hi")
  })
  test("http(s), file: and non-upload API paths are refused", async () => {
    for (const url of [
      "http://example.com/a.csv",
      "https://169.254.169.254/latest/meta-data",
      "file:///etc/passwd",
      "/api/v1/sessions/x",
      "/api/v1/uploads/../sessions",
      "//evil.example/api/v1/uploads/abc",
    ]) {
      await expect(readAttachment(url, signal)).rejects.toThrow(/only stages attachments sent in the chat/)
    }
  })
})

function ctx(callID: string, extra: Partial<Record<string, unknown>> = {}) {
  return {
    sessionID: "ses_run_code_test",
    messageID: "msg_1",
    callID,
    agent: "build",
    abort: AbortSignal.any([]),
    messages: [
      {
        info: { id: "msg_0", role: "user" },
        parts: [
          { id: "p0", type: "text", text: "plot column B" },
          {
            id: "p1",
            type: "file",
            mime: "text/csv",
            filename: "sales.csv",
            url: `data:text/csv;base64,${Buffer.from(CSV).toString("base64")}`,
          },
        ],
      },
    ],
    metadata: () => {},
    ask: async () => {},
    ...extra,
  } as unknown as Parameters<Awaited<ReturnType<typeof RunCodeTool.init>>["execute"]>[1]
}

describe.skipIf(skip)("tool.run_code", () => {
  test(
    "csv in → png + csv out, listed with media types",
    async () => {
      const tool = await RunCodeTool.init()
      const asks: Array<{ metadata: Record<string, unknown> }> = []
      const result = await tool.execute(
        {
          language: "python",
          code: [
            "import pandas as pd, matplotlib.pyplot as plt",
            "df = pd.read_csv('in/sales.csv')",
            "print('total', df['revenue'].sum())",
            "ax = df.plot(x='month', y='revenue', title='Revenue by month')",
            "ax.set_xlabel('Month'); ax.set_ylabel('Revenue ($k)'); ax.annotate('peak', (1, 30))",
            "plt.savefig('out/chart.png')",
            "df.assign(double=df.revenue * 2).to_csv('out/summary.csv', index=False)",
          ].join("\n"),
        },
        ctx("call_png", { ask: async (req: { metadata: Record<string, unknown> }) => void asks.push(req) }),
      )
      expect(result.metadata.stderr).not.toContain("Error")
      expect(result.metadata.exit_code).toBe(0)
      // The approval card gets the script itself.
      expect(asks[0]!.metadata.code).toContain("pd.read_csv('in/sales.csv')")
      expect(asks[0]!.metadata.code_truncated).toBe(false)
      expect(result.metadata.stdout).toContain("total 60")
      expect(result.metadata.inputs).toEqual(["sales.csv"])
      const files = result.metadata.files.map((f) => ({ name: f.name, mediaType: f.mediaType }))
      expect(files).toEqual([
        { name: "chart.png", mediaType: "image/png" },
        { name: "summary.csv", mediaType: "text/csv" },
      ])
      const png = await fs.readFile(result.metadata.files[0]!.path)
      expect(png.subarray(1, 4).toString()).toBe("PNG")
      expect(await fs.readFile(result.metadata.files[1]!.path, "utf8")).toContain("Feb,30,60")
      // allternit-api only ingests paths shaped like a run's out/ dir.
      expect(result.metadata.files[0]!.path).toContain(`${path.sep}gizzi-code${path.sep}runs${path.sep}`)
      expect(JSON.parse(result.output).files).toHaveLength(2)
      expect(result.metadata.skipped).toEqual([])
      expect(path.isAbsolute(result.metadata.runDir)).toBe(true)
      expect(result.metadata.files[0]!.path.startsWith(path.join(result.metadata.runDir, "out") + path.sep)).toBe(true)
    },
    15 * 60_000,
  )

  test(
    "network is off and the API environment is not passed in",
    async () => {
      process.env.RUN_CODE_TEST_SECRET = "s3cret"
      const tool = await RunCodeTool.init()
      const result = await tool.execute(
        {
          language: "python",
          code: [
            "import os, socket",
            "print('secret', os.environ.get('RUN_CODE_TEST_SECRET'))",
            "try:",
            "    socket.create_connection(('1.1.1.1', 443), timeout=3); print('net open')",
            "except OSError as e:",
            "    print('net blocked')",
          ].join("\n"),
        },
        ctx("call_net"),
      )
      delete process.env.RUN_CODE_TEST_SECRET
      expect(result.metadata.stdout).toContain("secret None")
      expect(result.metadata.stdout).toContain("net blocked")
    },
    15 * 60_000,
  )

  test("a missing named attachment says which files exist", async () => {
    const tool = await RunCodeTool.init()
    await expect(
      tool.execute({ language: "python", code: "print(1)", files: ["nope.csv"] }, ctx("call_missing")),
    ).rejects.toThrow(/No attachment named "nope.csv".*sales\.csv/)
  })

  test(
    "the script cannot read the user's home or write outside its run folder",
    async () => {
      const tag = `${process.pid}-${Date.now()}`
      const secret = path.join(os.homedir(), `.gizzi-run-code-secret-${tag}`)
      const outsideHome = path.join(os.homedir(), `.gizzi-run-code-write-${tag}`)
      const outsideTmp = path.join(os.tmpdir(), `gizzi-run-code-write-${tag}`)
      await fs.writeFile(secret, "TOP-SECRET")
      try {
        const tool = await RunCodeTool.init()
        const result = await tool.execute(
          {
            language: "python",
            code: [
              "import os",
              "def attempt(label, fn):",
              "    try:",
              "        fn(); print(label, 'ALLOWED')",
              "    except OSError as e:",
              "        print(label, 'denied', type(e).__name__)",
              `attempt('read-secret', lambda: print(open(${JSON.stringify(secret)}).read()))`,
              `attempt('list-ssh', lambda: os.listdir(${JSON.stringify(path.join(os.homedir(), ".ssh"))}))`,
              `attempt('list-home', lambda: os.listdir(${JSON.stringify(os.homedir())}))`,
              // World-readable outside the sandbox; denied inside it.
              "attempt('read-keychain', lambda: open('/Library/Keychains/System.keychain', 'rb').read(4))",
              "attempt('list-volumes', lambda: os.listdir('/Volumes'))",
              "attempt('list-users', lambda: os.listdir('/Users'))",
              `attempt('write-home', lambda: open(${JSON.stringify(outsideHome)}, 'w').write('x'))`,
              `attempt('write-tmp', lambda: open(${JSON.stringify(outsideTmp)}, 'w').write('x'))`,
              "attempt('write-out', lambda: open('out/ok.txt', 'w').write('x'))",
              "import tempfile; attempt('tempfile', lambda: tempfile.NamedTemporaryFile().close())",
            ].join("\n"),
          },
          ctx("call_strict"),
        )
        const out = result.metadata.stdout
        expect(out).not.toContain("TOP-SECRET")
        expect(out).toContain("read-secret denied")
        expect(out).toContain("list-ssh denied")
        expect(out).toContain("list-home denied")
        if (process.platform === "darwin") {
          expect(out).toContain("read-keychain denied")
          expect(out).toContain("list-volumes denied")
          expect(out).toContain("list-users denied")
        }
        expect(out).toContain("write-home denied")
        expect(out).toContain("write-tmp denied")
        expect(out).toContain("write-out ALLOWED")
        expect(out).toContain("tempfile ALLOWED")
        expect(await exists(outsideHome)).toBe(false)
        expect(await exists(outsideTmp)).toBe(false)
      } finally {
        await Promise.all([secret, outsideHome, outsideTmp].map((f) => fs.rm(f, { force: true })))
      }
    },
    15 * 60_000,
  )

  test("an http URL attachment is refused before anything runs", async () => {
    const tool = await RunCodeTool.init()
    const base = ctx("call_http")
    const messages = [
      {
        info: { id: "msg_h", role: "user" },
        parts: [{ id: "ph", type: "file", mime: "text/csv", filename: "remote.csv", url: "https://example.com/remote.csv" }],
      },
    ]
    await expect(
      tool.execute({ language: "python", code: "print(1)" }, { ...base, messages } as typeof base),
    ).rejects.toThrow(/Could not read the attachment "remote.csv".*only stages attachments sent in the chat/)
  })

  test(
    "files over the cap come back as metadata.skipped, not in stderr",
    async () => {
      const tool = await RunCodeTool.init()
      const result = await tool.execute(
        {
          language: "python",
          code: "for i in range(23):\n    open(f'out/f{i:02d}.txt', 'w').write(str(i))",
        },
        ctx("call_cap"),
      )
      expect(result.metadata.exit_code).toBe(0)
      expect(result.metadata.files).toHaveLength(20)
      expect(result.metadata.skipped).toEqual([
        { name: "f20.txt", reason: "over_limit" },
        { name: "f21.txt", reason: "over_limit" },
        { name: "f22.txt", reason: "over_limit" },
      ])
      expect(result.metadata.stderr).not.toContain("Not returned")
      expect(JSON.parse(result.output).skipped).toEqual(["f20.txt", "f21.txt", "f22.txt"])
    },
    15 * 60_000,
  )

  test(
    "the time limit stops a runaway script",
    async () => {
      const tool = await RunCodeTool.init()
      const result = await tool.execute(
        { language: "python", code: "import time\nprint('start', flush=True)\ntime.sleep(30)", timeout_s: 2 },
        ctx("call_timeout"),
      )
      expect(result.metadata.timed_out).toBe(true)
      expect(result.metadata.exit_code).toBeNull()
      expect(result.metadata.stderr).toContain("2s limit")
    },
    15 * 60_000,
  )
})
