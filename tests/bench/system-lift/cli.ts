import { resolve, join } from "node:path"
import { pathToFileURL } from "node:url"
import { benchmark, exportFixtures } from "./benchmark"
import { generateFixtures } from "./fixtures"
import { HttpModelPoolBackend, MockBackend } from "./backends"
import { MockBugFixGraph } from "./runners"
import { ROOT } from "./workspace"
import { GRAPH_ID, type Backend, type BugFixGraph } from "./types"

export async function main(argv = process.argv.slice(2)) {
  if (argv.includes("--help")) {
    console.log("bun tests/bench/system-lift/cli.ts [generate|run] --model-id <pool-backend-id> --backend <mock|http> --count <N> --seed <uint32> --out <worktree-path> [--gizzi-url <url> --graph-adapter <module.ts>] [--max-tokens <N> --max-calls <N> --timeout-ms <N> --bootstrap-samples <N>]")
    return
  }
  const action = argv[0] && !argv[0].startsWith("--") ? argv.shift()! : "run"
  const options: Record<string, string> = {}
  const allowed = new Set(["model-id", "backend", "count", "seed", "out", "gizzi-url", "graph-adapter", "max-tokens", "max-calls", "timeout-ms", "bootstrap-samples"])
  for (let i = 0; i < argv.length; i += 2) {
    const key = argv[i].replace(/^--/, ""), value = argv[i + 1]
    if (!argv[i].startsWith("--") || !allowed.has(key) || !value || value.startsWith("--") || key in options) throw new Error(`invalid CLI option ${argv[i]}`)
    options[key] = value
  }
  const integer = (name: string, fallback: number) => {
    const raw = options[name], value = raw === undefined ? fallback : Number(raw)
    if (!Number.isSafeInteger(value) || value < 0 || (raw !== undefined && !/^\d+$/.test(raw))) throw new Error(`${name} must be a nonnegative integer`)
    return value
  }
  const count = integer("count", 12), seed = integer("seed", 13), output = resolve(options.out ?? join(ROOT, ".tmp-wp13", "results"))
  if (action === "generate") { await exportFixtures(generateFixtures(count, seed), output); console.log(`Fixtures: ${output}`); return }
  if (action !== "run") throw new Error("action must be generate or run")
  if (!options["model-id"]) throw new Error("--model-id is required; no hard-coded model selection")
  const backendKind = options.backend ?? "mock"
  let backend: Backend
  if (backendKind === "mock") backend = new MockBackend(options["model-id"])
  else if (backendKind === "http") {
    if (!options["gizzi-url"]) throw new Error("--gizzi-url is required for HTTP model pool")
    // Reuse the operator's existing server credentials; never create a login or provider binding.
    const headers = JSON.parse(process.env.SYSTEM_LIFT_HTTP_HEADERS ?? "{}")
    if (!headers || Array.isArray(headers) || typeof headers !== "object" || Object.values(headers).some(x => typeof x !== "string")) throw new Error("HTTP headers must be a string-valued object")
    backend = new HttpModelPoolBackend(options["model-id"], options["gizzi-url"], headers)
  } else throw new Error("backend must be mock or http")
  let graph: BugFixGraph = new MockBugFixGraph()
  if (options["graph-adapter"]) {
    const path = resolve(options["graph-adapter"])
    if (!path.startsWith(ROOT + "/")) throw new Error("graph adapter must be inside this worktree")
    const module = await import(pathToFileURL(path).href)
    if (typeof module.createBugFixGraph !== "function") throw new Error("adapter must export createBugFixGraph")
    graph = await module.createBugFixGraph({ backendId: backend.id })
  } else if (backend.kind === "http") throw new Error("real system runs require --graph-adapter; offline graph is not WP10")
  if (!graph || graph.id !== GRAPH_ID || !graph.revision || typeof graph.production !== "boolean" || !graph.gates || typeof graph.execute !== "function") throw new Error("invalid BUG_FIX adapter")
  const report = await benchmark({ count, seed, backend, graph, output,
    budget: { maxTokens: integer("max-tokens", 100000), maxCalls: integer("max-calls", 2), timeoutMs: integer("timeout-ms", 60000) },
    bootstrapSamples: integer("bootstrap-samples", 2000) })
  console.log(`Report: ${join(output, "report.json")}\nTable: ${join(output, "report.md")}\nEligible: ${report.eligible}`)
}
if (import.meta.main) main().catch(error => { console.error(String(error)); process.exitCode = 1 })
