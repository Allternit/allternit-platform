import { afterEach, describe, expect, test } from "bun:test"
import { chmod, mkdtemp, rm, writeFile } from "node:fs/promises"
import path from "node:path"
import { tmpdir } from "node:os"
import { codexEngine } from "../../src/runtime/tools/builtins/codex-image"

const png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/l9sAAAAASUVORK5CYII="
const originalPath = process.env.PATH
const originalOverride = process.env.MULTICA_CODEX_PATH
let directory: string | undefined

// The only fake is the external Codex executable. Exercise the real stdio client and PNG boundary.
async function provider(mode: "image" | "text" | "signed-out" | "hang" | "outside" | "usage-limit" | "exit-after-turn" | "file-id", via: "path" | "override" = "path") {
  directory = await mkdtemp(path.join(tmpdir(), "codex-image-test-"))
  const binary = path.join(directory, "codex")
  await writeFile(binary, `#!${Bun.which("node")}\nimport {createInterface} from 'node:readline';
const send=x=>process.stdout.write(JSON.stringify(x)+'\\n');
createInterface({input:process.stdin}).on('line',line=>{
 const r=JSON.parse(line); if(!r.id)return;
 if(r.method==='initialize')send({id:r.id,result:{}});
 if(r.method==='account/read')send({id:r.id,result:{account:{type:${JSON.stringify(mode === "signed-out" ? "apiKey" : "chatgpt")}}}});
 if(r.method==='thread/start')send({id:r.id,result:{thread:{id:'thread-test'}}});
 if(r.method==='turn/start'){
   send({id:r.id,result:{turn:{id:'turn-test'}}});
   ${mode === "hang" ? "return;" : ""}
   ${mode === "exit-after-turn" ? "process.exit(1);" : ""}
   send({method:'item/completed',params:{threadId:'thread-test',item:${JSON.stringify(mode === "image" ? { type: "imageGeneration", id: "image-test", status: "completed", result: png } : mode === "usage-limit" ? { type: "imageGeneration", id: "image-test", status: "failed", failure: { type: "usageLimitExceeded", limitId: "l", resetsAt: 1800000000 } } : mode === "file-id" ? { type: "imageGeneration", id: "image-test", status: "completed", result: "", fileId: "file-1" } : mode === "outside" ? { type: "imageGeneration", id: "image-test", status: "completed", savedPath: "/etc/hosts" } : { type: "agentMessage", text: "I made an image at /tmp/output.png" })}}});
   send({method:'turn/completed',params:{threadId:'thread-test',turn:{status:'completed'}}});
 }
});\n`)
  await chmod(binary, 0o700)
  if (via === "override") {
    // Bare PATH: only the resolver's env override can find the binary.
    process.env.PATH = "/nonexistent-bin-dir"
    process.env.MULTICA_CODEX_PATH = binary
  } else process.env.PATH = `${directory}${path.delimiter}${originalPath ?? ""}`
}
afterEach(async () => { process.env.PATH = originalPath; if (originalOverride === undefined) delete process.env.MULTICA_CODEX_PATH; else process.env.MULTICA_CODEX_PATH = originalOverride; if (directory) await rm(directory, { recursive: true, force: true }); directory = undefined })

const run = (signal?: AbortSignal) => codexEngine.run({ prompt: "A blue mug", signal })

describe("Codex CLI picture engine", () => {
  test("returns the real PNG size from a completed imageGeneration item", async () => {
    await provider("image")
    expect(await run()).toMatchObject({ status: "ok", width: 1, height: 1 })
  })
  test("finds the binary off PATH through the resolver override", async () => {
    await provider("image", "override")
    expect(await run()).toMatchObject({ status: "ok" })
  })
  test("an agent's text response is not an image", async () => {
    await provider("text")
    expect(await run()).toMatchObject({ status: "definite", code: "unavailable" })
  })
  test("API-key authentication is not started", async () => {
    await provider("signed-out")
    expect(await run()).toMatchObject({ status: "not_started", reason: "signed_out" })
  })
  test("refuses image paths outside the artifact folders", async () => {
    await provider("outside")
    expect(await run()).toMatchObject({ status: "definite", code: "invalid_image" })
  })
  test("usage limit is definite with resetsAt in epoch ms", async () => {
    await provider("usage-limit")
    expect(await run()).toMatchObject({ status: "definite", code: "usage_limit", resetsAt: 1800000000000 })
  })
  test("a file id without bytes asks for a Codex update", async () => {
    await provider("file-id")
    const result = await run()
    expect(result).toMatchObject({ status: "definite", code: "unavailable" })
    expect((result as { error: string }).error).toContain("Update Codex")
  })
  test("process exit after turn/start is ambiguous", async () => {
    await provider("exit-after-turn")
    expect((await run()).status).toBe("ambiguous")
  })
  test("stop after turn/start is ambiguous", async () => {
    await provider("hang")
    const abort = new AbortController()
    const result = run(abort.signal)
    const timer = setTimeout(() => abort.abort(), 300)
    expect((await result).status).toBe("ambiguous")
    clearTimeout(timer)
  })
})
