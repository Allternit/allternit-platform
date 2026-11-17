// Wave C lane C: folder_access grants, the _to_delete redirect for deletes in
// granted outside folders, and the computer access ask + retry round trip.
import { describe, expect, test } from "bun:test"
import fs from "fs/promises"
import path from "path"
import { Instance } from "../../src/project/instance"
import { tmpdir } from "../fixture/fixture"
import { BashTool } from "../../src/runtime/tools/builtins/bash"
import { FolderAccessTool } from "../../src/runtime/tools/builtins/folder-access"
import { assertExternalDirectory } from "../../src/runtime/tools/builtins/external-directory"
import { PermissionNext } from "../../src/runtime/tools/guard/permission/next"
import { SessionSandbox } from "../../src/runtime/context/sandbox/session-sandbox"
import { runToolsetCall, COMPUTER_ACCESS_PERMISSION } from "../../src/runtime/tools/computer-toolset"

let n = 0
function makeCtx() {
  const asks: any[] = []
  const ctx = {
    sessionID: `ses_folder_access_${Date.now()}_${n++}`,
    messageID: `msg_${n}`,
    callID: "",
    agent: "build",
    abort: AbortSignal.any([]),
    messages: [],
    metadata: () => {},
    ask: async (req: any) => {
      asks.push(req)
    },
  }
  return { ctx, asks }
}

const exists = (p: string) =>
  fs.lstat(p).then(
    () => true,
    () => false,
  )

async function grant(ctx: any, folder: string, action: "request" | "request_delete" = "request") {
  const tool = await FolderAccessTool.init()
  return tool.execute({ action, paths: [folder], reason: "sort the photos" }, ctx)
}

async function rm(ctx: any, command: string) {
  const bash = await BashTool.init()
  return bash.execute({ command, description: "delete" }, ctx)
}

describe("folder_access", () => {
  test("always asks, then the folder skips the external_directory ask", async () => {
    await using project = await tmpdir({ git: true })
    await using outside = await tmpdir()
    await Instance.provide({
      directory: project.path,
      fn: async () => {
        const { ctx, asks } = makeCtx()
        expect(PermissionNext.ALWAYS_ASK.has("folder_access")).toBe(true)
        const result = await grant(ctx, outside.path)
        expect(asks).toHaveLength(1)
        expect(asks[0].permission).toBe("folder_access")
        expect(asks[0].patterns).toEqual([outside.path])
        expect(asks[0].metadata.folderAccess).toMatchObject({ action: "request", reason: "sort the photos" })
        expect(JSON.parse(result.output).granted).toEqual([outside.path])

        asks.length = 0
        await assertExternalDirectory(ctx, path.join(outside.path, "a.txt"))
        expect(asks).toHaveLength(0)
      },
    })
  })

  test("refuses the home folder and missing paths before asking", async () => {
    await using project = await tmpdir({ git: true })
    await Instance.provide({
      directory: project.path,
      fn: async () => {
        const { ctx, asks } = makeCtx()
        await expect(grant(ctx, "~")).rejects.toThrow("too broad")
        await expect(grant(ctx, path.join(project.path, "nope"))).rejects.toThrow("doesn't exist")
        expect(asks).toHaveLength(0)
      },
    })
  })
})

describe("deletes in a granted outside folder", () => {
  test("rm moves the file to <root>/_to_delete, keeping its relative path", async () => {
    await using project = await tmpdir({ git: true })
    await using outside = await tmpdir()
    await fs.mkdir(path.join(outside.path, "notes"))
    await fs.writeFile(path.join(outside.path, "notes", "a.txt"), "keep me")
    await Instance.provide({
      directory: project.path,
      fn: async () => {
        const { ctx } = makeCtx()
        await grant(ctx, outside.path)
        const result = await rm(ctx, `rm ${path.join(outside.path, "notes", "a.txt")}`)
        expect(result.metadata.exit).toBe(0)
        expect(await exists(path.join(outside.path, "notes", "a.txt"))).toBe(false)
        const moved = path.join(outside.path, "_to_delete", "notes", "a.txt")
        expect(await fs.readFile(moved, "utf8")).toBe("keep me")
        expect(result.output).toContain(moved)
      },
    })
  })

  test("a symlinked _to_delete is refused and nothing moves", async () => {
    await using project = await tmpdir({ git: true })
    await using outside = await tmpdir()
    await using elsewhere = await tmpdir()
    await fs.writeFile(path.join(outside.path, "a.txt"), "x")
    await fs.symlink(elsewhere.path, path.join(outside.path, "_to_delete"))
    await Instance.provide({
      directory: project.path,
      fn: async () => {
        const { ctx } = makeCtx()
        await grant(ctx, outside.path)
        await expect(rm(ctx, `rm ${path.join(outside.path, "a.txt")}`)).rejects.toThrow("not a plain folder")
        expect(await exists(path.join(outside.path, "a.txt"))).toBe(true)
        expect(await fs.readdir(elsewhere.path)).toEqual([])
      },
    })
  })

  test("a symlink inside _to_delete that leads out is refused", async () => {
    await using project = await tmpdir({ git: true })
    await using outside = await tmpdir()
    await using elsewhere = await tmpdir()
    await fs.mkdir(path.join(outside.path, "notes"))
    await fs.writeFile(path.join(outside.path, "notes", "a.txt"), "x")
    await fs.mkdir(path.join(outside.path, "_to_delete"))
    await fs.symlink(elsewhere.path, path.join(outside.path, "_to_delete", "notes"))
    await Instance.provide({
      directory: project.path,
      fn: async () => {
        const { ctx } = makeCtx()
        await grant(ctx, outside.path)
        await expect(rm(ctx, `rm ${path.join(outside.path, "notes", "a.txt")}`)).rejects.toThrow("not a plain folder")
        expect(await exists(path.join(outside.path, "notes", "a.txt"))).toBe(true)
        expect(await fs.readdir(elsewhere.path)).toEqual([])
      },
    })
  })

  test("a delete shape that can't be redirected is refused before it runs", async () => {
    await using project = await tmpdir({ git: true })
    await using outside = await tmpdir()
    await fs.writeFile(path.join(outside.path, "a.txt"), "x")
    await Instance.provide({
      directory: project.path,
      fn: async () => {
        const { ctx } = makeCtx()
        await grant(ctx, outside.path)
        await expect(rm(ctx, `rm ${outside.path}/*.txt`)).rejects.toThrow("request_delete")
        await expect(rm(ctx, `find ${outside.path} -name a.txt -delete`)).rejects.toThrow("Nothing ran")
        expect(await exists(path.join(outside.path, "a.txt"))).toBe(true)
      },
    })
  })

  test("request_delete makes deletes in the folder real", async () => {
    await using project = await tmpdir({ git: true })
    await using outside = await tmpdir()
    await fs.writeFile(path.join(outside.path, "a.txt"), "x")
    await Instance.provide({
      directory: project.path,
      fn: async () => {
        const { ctx, asks } = makeCtx()
        await SessionSandbox.disable(ctx.sessionID)
        await grant(ctx, outside.path)
        await grant(ctx, outside.path, "request_delete")
        expect(asks.filter((a) => a.permission === "folder_access")).toHaveLength(2)
        expect(asks[1].metadata.folderAccess.action).toBe("request_delete")
        const result = await rm(ctx, `rm ${path.join(outside.path, "a.txt")}`)
        expect(result.metadata.exit).toBe(0)
        expect(await exists(path.join(outside.path, "a.txt"))).toBe(false)
        expect(await exists(path.join(outside.path, "_to_delete"))).toBe(false)
      },
    })
  })

  test("deletes in the project are unchanged while a folder is granted", async () => {
    await using project = await tmpdir({ git: true })
    await using outside = await tmpdir()
    await fs.writeFile(path.join(project.path, "tmp.txt"), "x")
    await Instance.provide({
      directory: project.path,
      fn: async () => {
        const { ctx } = makeCtx()
        await SessionSandbox.disable(ctx.sessionID)
        await grant(ctx, outside.path)
        const result = await rm(ctx, "rm tmp.txt")
        expect(result.metadata.exit).toBe(0)
        expect(await exists(path.join(project.path, "tmp.txt"))).toBe(false)
        expect(await exists(path.join(project.path, "_to_delete"))).toBe(false)
      },
    })
  })
})

// The executor (allternit-api) is the network edge: a scripted fake stands in.
function executor(replies: Array<(req: any) => { status: number; body: any }>) {
  const runs: any[] = []
  const approved: string[] = []
  return {
    runs,
    approved,
    client: {
      async run(_id: string, req: any) {
        runs.push(req)
        const next = replies.shift()
        if (!next) throw new Error(`unexpected call ${req.member}`)
        return next(req)
      },
      async schema() {
        return undefined
      },
      async approve(id: string) {
        approved.push(id)
        return true
      },
    },
  }
}

const screen = { width: 1440, height: 900, scale: 1 }
const ok = (text: string) => ({ status: 200, body: { is_error: false, content: [{ type: "text", text }], screen } })

describe("computer access asks", () => {
  test("access_required asks once, grants with request_access, and retries the step", async () => {
    const { ctx, asks } = makeCtx()
    const fake = executor([
      () => ({
        status: 403,
        body: {
          is_error: true,
          error: "access_required",
          content: [{ type: "text", text: "Notes has read access in this session and this step needs click." }],
          screen,
          access: {
            needed_tier: "click",
            granted_tier: "read",
            entries: [{ name: "Notes", bundle_id: "com.apple.Notes", category: "other", max_tier: "full" }],
          },
        },
      }),
      (req) => ({
        status: 409,
        body: { is_error: true, error: "approval_required", approval_id: "ap_1", content: [], screen, access: { entries: req.input.entries } },
      }),
      (req) => (req.approval_grant === "ap_1" ? ok('{"granted":[{"bundle_id":"com.apple.Notes","tier":"click"}]}') : { status: 409, body: { error: "approval_required" } }),
      () => ok("Clicked."),
    ])
    const out = await runToolsetCall(
      "computer",
      { action: "left_click", coordinate: [10, 10], step: "open the note" },
      ctx,
      { executor: fake.client, computerId: "this-device", grid: false },
    )
    expect(out.output).toBe("Clicked.")
    expect(asks).toHaveLength(1)
    expect(asks[0].permission).toBe(COMPUTER_ACCESS_PERMISSION)
    expect(asks[0].metadata.access.entries).toEqual([
      { name: "Notes", bundle_id: "com.apple.Notes", category: "other", max_tier: "full", tier: "click" },
    ])
    expect(asks[0].metadata.access.reason).toBe("open the note")
    // What reached the executor: the grant request, its approved retry, then the step again.
    expect(fake.runs.map((r) => r.member)).toEqual(["left_click", "request_access", "request_access", "left_click"])
    expect(fake.runs[1].input).toEqual({ entries: [{ bundle_id: "com.apple.Notes", tier: "click" }], reason: "open the note" })
    expect(fake.runs[1].call_index).toBe(fake.runs[0].call_index)
    expect(fake.approved).toEqual(["ap_1"])
  })

  test("request_access shows apps, tiers and reason, then retries with the approval grant", async () => {
    const { ctx, asks } = makeCtx()
    const fake = executor([
      () => ({
        status: 409,
        body: {
          is_error: true,
          error: "approval_required",
          approval_id: "ap_2",
          content: [],
          screen,
          access: {
            entries: [{ name: "Xcode", bundle_id: "com.apple.dt.Xcode", category: "ide", max_tier: "click", tier: "click" }],
            reason: "run the tests",
            expires_in_secs: 43200,
          },
        },
      }),
      (req) => (req.approval_grant === "ap_2" ? ok('{"granted":[{"bundle_id":"com.apple.dt.Xcode","tier":"click"}]}') : { status: 409, body: {} }),
    ])
    const out = await runToolsetCall(
      "computer",
      { action: "request_access", entries: [{ bundle_id: "com.apple.dt.Xcode", tier: "click" }], reason: "run the tests" },
      ctx,
      { executor: fake.client, computerId: "this-device", grid: false },
    )
    expect(JSON.parse(out.output).granted[0].tier).toBe("click")
    expect(asks).toHaveLength(1)
    expect(asks[0].permission).toBe("computer.access")
    expect(asks[0].patterns).toEqual(["com.apple.dt.Xcode:click"])
    expect(asks[0].metadata.access).toMatchObject({ reason: "run the tests", entries: [{ name: "Xcode", tier: "click" }] })
  })

  test("tier_exceeded comes back as a clear error and asks nothing", async () => {
    const { ctx, asks } = makeCtx()
    const fake = executor([
      () => ({
        status: 403,
        body: {
          is_error: true,
          error: "tier_exceeded",
          content: [{ type: "text", text: "Safari is a browser app; it can be granted at most read access, not click. Ask for read or less." }],
          screen,
        },
      }),
    ])
    const call = runToolsetCall(
      "computer",
      { action: "request_access", entries: [{ bundle_id: "com.apple.Safari", tier: "click" }], reason: "click a link" },
      ctx,
      { executor: fake.client, computerId: "this-device", grid: false },
    )
    await expect(call).rejects.toThrow(/tier_exceeded: Safari is a browser app.*resolve_access/)
    expect(asks).toHaveLength(0)
  })

  test("access_required above the app's ceiling asks nothing", async () => {
    const { ctx, asks } = makeCtx()
    const fake = executor([
      () => ({
        status: 403,
        body: {
          is_error: true,
          error: "access_required",
          content: [{ type: "text", text: "Terminal is a terminal app; agents can use it at most at click access, and this step needs full." }],
          screen,
          access: {
            needed_tier: "full",
            granted_tier: "click",
            entries: [{ name: "Terminal", bundle_id: "com.apple.Terminal", category: "terminal", max_tier: "click" }],
          },
        },
      }),
    ])
    const call = runToolsetCall("computer", { action: "type", text: "ls" }, ctx, {
      executor: fake.client,
      computerId: "this-device",
      grid: false,
    })
    await expect(call).rejects.toThrow("at most at click")
    expect(asks).toHaveLength(0)
    expect(fake.runs).toHaveLength(1)
  })
})
