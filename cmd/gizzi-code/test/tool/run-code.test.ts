import { describe, expect, test } from "bun:test"
import os from "os"
import path from "path"
import fs from "fs/promises"
import { RunCodeTool } from "../../src/runtime/tools/builtins/run_code"

// The first run sets up the real uv venv (network). Reuse one env dir across
// local runs; CI without network sets GIZZI_SKIP_RUN_CODE_E2E=1.
process.env.GIZZI_PYTHON_ENV_DIR ??= path.join(os.tmpdir(), "gizzi-run-code-test-python")
const skip = process.env.GIZZI_SKIP_RUN_CODE_E2E === "1" || process.platform === "win32"

const CSV = "month,revenue\nJan,10\nFeb,30\nMar,20\n"

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
      const result = await tool.execute(
        {
          language: "python",
          code: [
            "import pandas as pd, matplotlib.pyplot as plt",
            "df = pd.read_csv('in/sales.csv')",
            "print('total', df['revenue'].sum())",
            "df.plot(x='month', y='revenue'); plt.savefig('out/chart.png')",
            "df.assign(double=df.revenue * 2).to_csv('out/summary.csv', index=False)",
          ].join("\n"),
        },
        ctx("call_png"),
      )
      expect(result.metadata.exit_code).toBe(0)
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
