import { afterEach, describe, expect, test } from "bun:test"
import { chmod, mkdtemp, rm, writeFile } from "node:fs/promises"
import path from "node:path"
import { tmpdir } from "node:os"
import { generateCodexImage } from "../../src/runtime/tools/builtins/codex-image"
import { browserImageResult } from "../../src/runtime/tools/builtins/media-generate"

const png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/l9sAAAAASUVORK5CYII="
const originalPath = process.env.PATH
let directory: string | undefined

// The only fake is the external Codex executable. Exercise the real stdio client and PNG boundary.
async function provider(mode: "image" | "text" | "signed-out" | "hang" | "outside") {
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
   send({method:'item/completed',params:{threadId:'thread-test',item:${JSON.stringify(mode === "image" ? { type: "imageGeneration", id: "image-test", status: "completed", result: png } : mode === "outside" ? { type: "imageGeneration", id: "image-test", status: "completed", savedPath: "/etc/hosts" } : { type: "agentMessage", text: "I made an image at /tmp/output.png" })}}});
   send({method:'turn/completed',params:{threadId:'thread-test',turn:{status:'completed'}}});
 }
});\n`)
  await chmod(binary, 0o700)
  process.env.PATH = `${directory}${path.delimiter}${originalPath ?? ""}`
  expect(Bun.which("codex", { PATH: process.env.PATH })).toBe(binary)
}
afterEach(async () => { process.env.PATH = originalPath; if (directory) await rm(directory, { recursive: true, force: true }); directory = undefined })

describe("Codex CLI image protocol", () => {
  test("returns a validated image from a completed imageGeneration item", async () => {
    await provider("image")
    const result = await generateCodexImage("A blue mug", new AbortController().signal, () => {})
    expect(result).toMatchObject({ ok: true })
    expect(browserImageResult(result)).toMatchObject({ width: 1, height: 1 })
  })
  test("does not pass off an agent's text response as an image", async () => {
    await provider("text")
    const result = await generateCodexImage("A blue mug", new AbortController().signal, () => {})
    expect(result.ok).toBe(false)
    expect(result.image).toBeUndefined()
  })
  test("refuses API-key authentication instead of silently billing it", async () => {
    await provider("signed-out")
    expect(await generateCodexImage("A blue mug", new AbortController().signal, () => {})).toMatchObject({ ok: false, code: "signed_out" })
  })
  test("refuses image paths outside the artifact folders", async () => {
    await provider("outside")
    const result = await generateCodexImage("A blue mug", new AbortController().signal, () => {})
    expect(result.ok).toBe(false)
    expect(result.image).toBeUndefined()
  })
  test("cancels the owned CLI process and settles the job", async () => {
    await provider("hang")
    const abort = new AbortController()
    const result = generateCodexImage("A blue mug", abort.signal, () => {})
    const timer = setTimeout(() => abort.abort(), 100)
    expect(await result).toMatchObject({ ok: false, code: "cancelled" })
    clearTimeout(timer)
  })
})
