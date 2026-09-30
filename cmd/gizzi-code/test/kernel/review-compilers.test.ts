import { describe, expect, test } from "bun:test"
import { compileToolCall, ToolCompileError, type OperationDescriptor } from "../../src/runtime/kernel/compilers/tool-call-compiler"
import { compileContext } from "../../src/runtime/kernel/compilers/context-compiler"
import { KernelTurn } from "../../src/runtime/kernel/compilers/turn-hook"
import { sha256Tagged } from "../../src/runtime/kernel/compilers/jcs"

const NOW = "2026-09-30T12:00:00Z"
const state = KernelTurn.sessionState({ sessionID: "review", directory: "/repo", objective: "x", stateVersion: 1 })
const op: OperationDescriptor = {
  tool_id: "write", operation_id: "invoke", summary: "Write file", effect_class: "WORKSPACE_WRITE",
  params: [{ name: "path", type: "string", required: true, resource: "fs", enum: ["/repo/a"] }],
}
function fixed(value: unknown) {
  return compileToolCall({ state, op, bindings: {}, policy: { policy_decision_id: "pd1", environment_id: "e1", fixed: { path: value } }, now: NOW })
}
describe("review #21 policy-fixed arguments", () => {
  test("policy-fixed filesystem parameter still obeys required type and enum", () => {
    for (const value of [123, "/outside/enum", undefined]) {
      expect(() => fixed(value)).toThrow(ToolCompileError)
    }
    const good = fixed("/repo/a")
    expect((good.invocation.arguments as { path?: unknown }).path).toBe("/repo/a")
    expect(good.invocation.argument_provenance.path).toBe("POLICY")
    expect(good.invocation.write_set).toEqual(["fs:/repo/a"])
  })
  test("policy override validates the winning value, including policy-only parameters", () => {
    const input = { state, op: { ...op, params: [{ ...op.params[0], policy_only: true }] }, bindings: { path: { class: "GENERATED" as const, value: 123 } }, policy: { policy_decision_id: "pd1", environment_id: "e1", fixed: { path: "/repo/a" } }, now: NOW }
    expect(compileToolCall(input).diagnostics).toEqual(["POLICY_OVERRIDE:path:GENERATED"])
    expect(() => compileToolCall({ ...input, policy: { ...input.policy, fixed: { path: 123 } } })).toThrow("SCHEMA_VIOLATION")
  })
})

describe("review #22 serialized context identity", () => {
  function compile(short: string, trust_class: "INTERNAL" | "UNTRUSTED" = "INTERNAL", source_id = "source") {
    return compileContext({ state, node: { node_id: "n", capability: "cap.edit", next_op: "edit" }, now: NOW, budget_tokens: 10,
      candidates: [{ key: "source", kind: "FILE", text: "x".repeat(200), short, source: "FILE", source_id, trust_class }] })
  }
  test("equal-length SHORT summaries have distinct fingerprints and content references", () => {
    const a = compile("first"), b = compile("other")
    const chunk = (o: typeof a) => o.chunks.find((c) => (c.extensions!["x-item"] as any).source_key === "source")!
    const ac = chunk(a), bc = chunk(b)
    expect(a.projection.selected.find((d) => d.chunk_id === ac.chunk_id)!.visibility).toBe("SHORT")
    expect(a.rendered).not.toBe(b.rendered)
    expect(a.projection.context_fingerprint).not.toBe(b.projection.context_fingerprint)
    expect(a.projection.projection_id).not.toBe(b.projection.projection_id)
    expect(ac.content_hash).toBe(sha256Tagged("first"))
    expect(bc.content_hash).toBe(sha256Tagged("other"))
    expect(ac.content_ref).toBe(`cas:${sha256Tagged("first")}`)
    expect(ac.source_ref.content_hash).toBe(sha256Tagged("x".repeat(200)))
    expect(Object.values(ac.token_estimates)).toEqual([Math.ceil("first".length / 4)])
    expect(a.projection.context_fingerprint).toBe(compile("first").projection.context_fingerprint)
  })
  test("fingerprint includes serialized fencing and source headers", () => {
    const a = compile("first"), fenced = compile("first", "UNTRUSTED"), moved = compile("first", "INTERNAL", "other-source")
    expect(a.rendered).not.toBe(fenced.rendered)
    expect(a.projection.context_fingerprint).not.toBe(fenced.projection.context_fingerprint)
    expect(a.rendered).not.toBe(moved.rendered)
    expect(a.projection.context_fingerprint).not.toBe(moved.projection.context_fingerprint)
  })
})
