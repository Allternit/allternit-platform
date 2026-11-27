// @ts-nocheck
import { test, expect, afterEach } from "bun:test"
import fs from "fs/promises"
import os from "os"
import path from "path"
import { spawnSync } from "child_process"
import { Instance } from "../../src/runtime/context/project/instance"
import { Session } from "../../src/runtime/session"
import { SessionTrace } from "../../src/runtime/session/trace"
import { Identifier } from "../../src/shared/id/id"
import { EvalStore } from "../../src/runtime/eval/store"
import { runSave, runList, runRemove, runExport, EvalCliError } from "../../src/cli/commands/eval"
import { parseTestcaseArgs } from "../../src/cli/ui/ink-app/commands/testcase/testcase"
import { tmpdir } from "../fixture/fixture"

const created: string[] = []
afterEach(async () => {
  for (const id of created.splice(0)) await EvalStore.deleteCase(id).catch(() => {})
})

async function makeSession(prompt = "Fix the failing test") {
  const session = await Session.create({ title: "eval cli test" })
  const t0 = Date.now()
  const messageID = Identifier.ascending("message")
  await Session.updateMessage({
    id: messageID,
    sessionID: session.id,
    role: "user",
    time: { created: t0 },
    agent: "build",
    model: { providerID: "test", modelID: "test" },
  })
  await Session.updatePart({ id: Identifier.ascending("part"), sessionID: session.id, messageID, type: "text", text: prompt })
  SessionTrace.append({ sessionID: session.id, kind: "request.started", data: { agent: "build" }, time: t0 })
  SessionTrace.append({ sessionID: session.id, kind: "request.completed", data: { cost: 0.01, tokens: { input: 1, output: 1 } }, time: t0 + 2000 })
  return session
}

async function inProject<T>(fn: () => Promise<T>) {
  await using tmp = await tmpdir({ git: true })
  return Instance.provide({ directory: tmp.path, fn })
}

const collect = () => {
  const lines: string[] = []
  return { lines, out: (l: string) => lines.push(l) }
}

test("save prints the id, list shows it, rm removes it", async () => {
  await inProject(async () => {
    const s = await makeSession()
    const saved = collect()
    await runSave({ sessionID: s.id, good: true, note: "works" }, saved.out)
    const id = /Saved test case (case_\w+) \(my-cases\)/.exec(saved.lines[0])?.[1]
    expect(id).toBeDefined()
    created.push(id!)
    expect(saved.lines[1]).toMatch(/^(Self-test: agrees|Needs a check: .+)$/)

    const listed = collect()
    await runList({ session: s.id }, listed.out)
    const row = listed.lines.find((l) => l.startsWith(id!))
    expect(row).toContain("Fix the failing test")
    expect(listed.lines[0]).toContain("NEEDS CHECK")

    const removed = collect()
    await runRemove({ id: id! }, removed.out)
    expect(removed.lines[0]).toBe(`Removed test case ${id}`)
    const after = collect()
    await runList({ session: s.id }, after.out)
    expect(after.lines).toEqual(["No test cases saved yet."])
  })
})

test("save needs exactly one label and shows a plain error", async () => {
  await expect(runSave({ sessionID: "x" }, () => {})).rejects.toThrow(EvalCliError)
  await expect(runSave({ sessionID: "x", good: true, bad: true }, () => {})).rejects.toThrow("Pick one")
  await inProject(async () => {
    await expect(runSave({ sessionID: "ses_missing", bad: true }, () => {})).rejects.toThrow("That session was not found.")
  })
})

test("export refuses when a planted key is in the workspace tar, and writes a clean copy otherwise", async () => {
  await inProject(async () => {
    const s = await makeSession()
    const saved = await EvalStore.saveCase({ sessionID: s.id, label: "bad", surface: "cli" })
    created.push(saved.case.id)
    const caseDir = path.join(EvalStore.root(), "suites", "my-cases", "cases", saved.case.id)
    const work = await fs.mkdtemp(path.join(os.tmpdir(), "eval-cli-"))
    try {
      const src = path.join(work, "src")
      await fs.mkdir(src)
      await fs.writeFile(path.join(src, ".env"), "OPENAI_API_KEY=sk-abcdefghijklmnopqrstuvwx1234\n")
      expect(spawnSync("tar", ["-czf", path.join(caseDir, "workspace.tar.gz"), "-C", src, "."]).status).toBe(0)

      const dest = path.join(work, "out")
      await expect(runExport({ id: saved.case.id, out: dest }, () => {})).rejects.toThrow(/Export stopped: 1 files look like secrets: .*\.env/)
      expect(await fs.stat(path.join(dest, saved.case.id)).catch(() => null)).toBeNull()

      await fs.writeFile(path.join(src, ".env"), "NAME=demo\n")
      expect(spawnSync("tar", ["-czf", path.join(caseDir, "workspace.tar.gz"), "-C", src, "."]).status).toBe(0)
      const done = collect()
      await runExport({ id: saved.case.id, out: dest }, done.out)
      const files = (await fs.readdir(path.join(dest, saved.case.id))).sort()
      expect(files).toEqual(["case.json", "redaction_report.json", "trace.json", "workspace.tar.gz"])
    } finally {
      await fs.rm(work, { recursive: true, force: true })
    }
  })
})

test("/testcase saves as bad when no label is given", () => {
  expect(parseTestcaseArgs("")).toEqual({ label: "bad", note: undefined })
  expect(parseTestcaseArgs("good It was fine")).toEqual({ label: "good", note: "It was fine" })
  expect(parseTestcaseArgs("it deleted my file")).toEqual({ label: "bad", note: "it deleted my file" })
  expect(parseTestcaseArgs("bad")).toEqual({ label: "bad", note: undefined })
})
