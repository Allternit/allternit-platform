import { sourcePatch } from "./fixtures"
import { emptyUsage, type Backend, type Completion, type ModelRequest, type Usage } from "./types"

/** Rules over visible source, deliberately missing async-await on the first shot. No oracle access. */
export class MockBackend implements Backend {
  readonly kind = "mock" as const
  constructor(readonly id: string) {}
  async complete(request: ModelRequest): Promise<Completion> {
    if (request.backendId !== this.id) throw new Error("backend mismatch")
    const before = request.files["src/lib.ts"]
    let after = before.replace("i < n", "i <= n")
      .replace("value!.trim()", "value?.trim() ?? ''")
      .replace("multiply as operation", "add as operation")
      .replace("[...values].sort()", "[...values].sort((a, b) => a - b)")
      .replace("value || fallback", "value ?? fallback")
    if (request.feedback) after = after.replace("const value = readLabel()", "const value = await readLabel()")
    const patch = before === after ? "" : sourcePatch(before, after)
    return { patch, usage: { ...emptyUsage(), input: Math.ceil(JSON.stringify(request.files).length / 4),
      output: Math.ceil(patch.length / 4), estimated: true } }
  }
}

type Fetch = typeof fetch
/** Resolve only registry IDs. All inference goes through the existing gizzi HTTP session routes. */
export class HttpModelPoolBackend implements Backend {
  readonly kind = "http" as const
  constructor(readonly id: string, private readonly base: string, private readonly headers: Record<string, string> = {},
    private readonly transport: Fetch = fetch) {
    const url = new URL(base)
    if (!["http:", "https:"].includes(url.protocol) || url.username || url.password || url.search || url.hash) throw new Error("invalid gizzi HTTP URL")
  }
  private async request(path: string, method: string, body: unknown, signal: AbortSignal) {
    const response = await this.transport(this.base.replace(/\/$/, "") + path, {
      method, headers: { ...this.headers, "content-type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body), signal,
    })
    if (!response.ok) throw new Error(`gizzi HTTP ${response.status} at ${path.split('?')[0]}`)
    return response.json() as Promise<any>
  }
  async complete(input: ModelRequest): Promise<Completion> {
    if (input.backendId !== this.id) throw new Error("backend mismatch")
    const signal = AbortSignal.timeout(input.budget.timeoutMs)
    const pool = await this.request("/model-pool?role=S2&capability=cap.code.edit", "GET", undefined, signal)
    const entry = pool.entries?.find((x: any) => x.backend_id === this.id)
    if (!entry || entry.schema_version !== "1.0.0") throw new Error("backend not in code-edit model pool")
    const ref = entry.extensions?.["x-model_ref"]
    if (typeof ref !== "string" || ref.indexOf("/") < 1 || ref.endsWith("/")) throw new Error("pool entry has no model reference")
    const slash = ref.indexOf("/"), model = { providerID: ref.slice(0, slash), modelID: ref.slice(slash + 1) }
    const ids = await this.request("/experimental/tool/ids", "GET", undefined, signal)
    if (!Array.isArray(ids) || ids.some(x => typeof x !== "string")) throw new Error("invalid tool registry")
    const tools = Object.fromEntries(ids.map((id: string) => [id, false]))
    const session = await this.request("/session", "POST", {
      title: "system-lift isolated generation", model,
      permission: [{ permission: "*", pattern: "*", action: "deny" }],
    }, signal)
    if (typeof session.id !== "string") throw new Error("missing session ID")
    const path = `/session/${encodeURIComponent(session.id)}`
    try {
      const reply = await this.request(path + "/message", "POST", {
        model, fallbackModels: [], tools,
        system: "Produce one bounded unified diff modifying only src/lib.ts. Return only the diff. No tools, planning, delegation or extra turns.",
        parts: [{ type: "text", text: JSON.stringify({ bug_report: input.report, repo: input.files, feedback: input.feedback,
          max_tokens: input.budget.maxTokens }) }],
      }, signal)
      if (reply.info?.error) throw new Error("generation failed")
      if (reply.info?.providerID !== model.providerID || reply.info?.modelID !== model.modelID) throw new Error("model substituted")
      if (!Array.isArray(reply.parts) || reply.parts.some((p: any) => p.type === "tool") ||
        reply.parts.filter((p: any) => p.type === "step-finish").length > 1) throw new Error("generation was not one-shot")
      const messages = await this.request(path + "/messages", "GET", undefined, signal)
      if (!Array.isArray(messages) || messages.filter((m: any) => m.info?.role === "assistant").length !== 1) throw new Error("generation used multiple turns")
      const tokens = reply.info?.tokens
      const values = [tokens?.input, tokens?.output, tokens?.reasoning, tokens?.cache?.read, tokens?.cache?.write]
      if (values.some(x => !Number.isFinite(x) || x < 0)) throw new Error("token telemetry missing")
      const usage: Usage = { input: values[0], output: values[1], reasoning: values[2], cacheRead: values[3], cacheWrite: values[4], estimated: !!reply.info.tokensEstimated }
      let patch = reply.parts.filter((p: any) => p.type === "text").map((p: any) => p.text).join("\n").trim()
      if (/^```(?:diff)?\n[\s\S]*\n```$/.test(patch)) patch = patch.replace(/^```(?:diff)?\n/, "").replace(/\n```$/, "")
      return { patch: patch ? patch + "\n" : "", usage }
    } finally {
      // Abort/delete only this call's fresh session, including when HTTP times out.
      const cleanupSignal = AbortSignal.timeout(5000)
      await this.request(path + "/abort", "POST", {}, cleanupSignal).catch(() => {})
      await this.request(path, "DELETE", undefined, cleanupSignal).catch(() => {})
    }
  }
}
