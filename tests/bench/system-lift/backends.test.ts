import { expect, test } from "bun:test"
import { generateFixtures } from "./fixtures"
import { HttpModelPoolBackend, MockBackend } from "./backends"
import type { ModelRequest } from "./types"

const request: ModelRequest = { backendId: "be.test", files: generateFixtures(1)[0].files,
  report: "fix endpoint", attempt: 1, budget: { maxTokens: 10000, maxCalls: 2, timeoutMs: 10000 } }
function fakeHttp(options: { missing?: boolean; substitute?: boolean; turns?: number; status?: number; noTokens?: boolean; tool?: boolean } = {}) {
  const calls: { path: string; method: string; body: any }[] = []
  const info = { role: "assistant", providerID: options.substitute ? "other-route" : "route", modelID: "fixture-model/variant",
    tokens: options.noTokens ? undefined : { input: 10, output: 20, reasoning: 3, cache: { read: 4, write: 5 } } }
  const transport = (async (url: string | URL | Request, init: RequestInit) => {
    const path = new URL(String(url)).pathname
    const body = init.body ? JSON.parse(init.body as string) : undefined
    calls.push({ path, method: init.method!, body })
    if (options.status) return new Response("failed", { status: options.status })
    let response: any
    if (path === "/model-pool") response = { entries: options.missing ? [] : [{ schema_version: "1.0.0", backend_id: "be.test", extensions: { "x-model_ref": "route/fixture-model/variant" } }] }
    else if (path === "/experimental/tool/ids") response = ["bash", "edit", "task"]
    else if (path === "/session") response = { id: "session-test" }
    else if (path.endsWith("/message")) response = { info, parts: [{ type: "text", text: "```diff\ndiff --git a/src/lib.ts b/src/lib.ts\n```" }, ...(options.tool ? [{ type: "tool" }] : [])] }
    else if (path.endsWith("/messages")) response = Array.from({ length: options.turns ?? 1 }, () => ({ info }))
    else response = true
    return Response.json(response)
  }) as typeof fetch
  return { calls, transport }
}
test("HTTP resolves pool ID, disables advertised tools, uses one fresh session and collects usage", async () => {
  const http = fakeHttp()
  const backend = new HttpModelPoolBackend("be.test", "http://gizzi.invalid", { authorization: "existing-test-session" }, http.transport)
  const result = await backend.complete(request)
  expect(result.usage).toEqual({ input: 10, output: 20, reasoning: 3, cacheRead: 4, cacheWrite: 5, estimated: false })
  expect(result.patch).toBe("diff --git a/src/lib.ts b/src/lib.ts\n")
  const prompt = http.calls.find(x => x.path.endsWith("/message"))!.body
  expect(prompt.model).toEqual({ providerID: "route", modelID: "fixture-model/variant" })
  expect(prompt.fallbackModels).toEqual([])
  expect(prompt.tools).toEqual({ bash: false, edit: false, task: false })
  expect(JSON.stringify(prompt)).not.toContain("fixedSource")
  expect(http.calls.slice(-2).map(x => x.method)).toEqual(["POST", "DELETE"])
  expect(http.calls.every(x => !x.path.includes("auth"))).toBe(true)
})
test("HTTP refuses unknown backbones, substitution, loops, tool calls and missing telemetry", async () => {
  for (const options of [{ missing: true }, { substitute: true }, { turns: 2 }, { tool: true }, { noTokens: true }, { status: 503 }]) {
    const http = fakeHttp(options)
    const backend = new HttpModelPoolBackend("be.test", "http://gizzi.invalid", {}, http.transport)
    await expect(backend.complete(request)).rejects.toThrow()
    if (http.calls.some(x => x.path === "/session")) expect(http.calls.at(-1)!.method).toBe("DELETE")
  }
})
test("mock is deterministic and requires verification feedback for async repair", async () => {
  const mock = new MockBackend("be.test"), files = generateFixtures(4)[3].files
  const first = await mock.complete({ ...request, files })
  expect(first).toEqual(await mock.complete({ ...request, files }))
  expect(first.patch).toBe("")
  const next = await mock.complete({ ...request, files, attempt: 2, feedback: "target test failed" })
  expect(next.patch).toContain("await readLabel()")
  await expect(mock.complete({ ...request, backendId: "other" })).rejects.toThrow()
})
test("graph hypothesis and review nodes can use the same HTTP backbone without patch parsing", async () => {
  const http = fakeHttp()
  const backend = new HttpModelPoolBackend("be.test", "http://gizzi.invalid", {}, http.transport)
  const response = await backend.complete({ ...request, stage: "N09", instruction: "Return a repair hypothesis as JSON", responseFormat: "text" })
  expect(response.patch).toBe("")
  expect(response.text).toContain("```diff")
  expect(http.calls.find(x => x.path.endsWith("/message"))!.body.system).toContain("repair hypothesis")
})
