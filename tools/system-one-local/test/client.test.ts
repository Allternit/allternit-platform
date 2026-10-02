import { describe, expect, test } from "bun:test";
import { decide, gateRecommendation, gateRequest, reportOutcome, shadowGate, tighten, type Friction } from "../src/decision/client.ts";
import { GUARD_GATE, handleOutcomeHook, harnessIncumbent, OUTCOME_SOURCES, reportToolRan, runGuard } from "../src/hook/guard.ts";
import { existsSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join as joinPath } from "node:path";

const ALL: Friction[] = ["allow", "ask", "deny"];

describe("tighten-only invariant (Q26: S1 may only tighten permission decisions)", () => {
  test("an S1 allow can never turn ask/deny into allow", () => {
    expect(tighten("ask", "allow")).toBe("ask");
    expect(tighten("deny", "allow")).toBe("deny");
  });
  test("exhaustive: result is never looser than the incumbent, and missing S1 leaves it unchanged", () => {
    const rank = { allow: 0, ask: 1, deny: 2 };
    for (const inc of ALL) {
      for (const s1 of [...ALL, null, undefined, "bogus" as any]) {
        const out = tighten(inc, s1);
        expect(rank[out]).toBeGreaterThanOrEqual(rank[inc]);
        if (s1 === null || s1 === undefined || s1 === "bogus") expect(out).toBe(inc);
      }
    }
    expect(tighten("allow", "ask")).toBe("ask");
    expect(tighten("ask", "deny")).toBe("deny");
  });
  test("recommendation thresholds", () => {
    expect(gateRecommendation(0.95)).toBe("allow");
    expect(gateRecommendation(0.5)).toBe("ask");
    expect(gateRecommendation(0.1)).toBe("deny");
    expect(gateRecommendation(null)).toBeNull();
  });
});

type Call = { url: string; body: any };
const fakeRuntime = (pTrue: number, calls: Call[]) => async (url: string, init?: RequestInit) => {
  const body = JSON.parse(String(init?.body));
  calls.push({ url, body });
  if (url.endsWith("/v1/decision/outcome")) return new Response("{}");
  const r = body.request;
  const probabilities = r.operation === "SCORE" ? Object.fromEntries(r.scale.map((_: string, i: number) => [String(i), i === 0 ? 1 : 0])) : r.operation === "GATE" ? { true: pTrue, false: 1 - pTrue } : { true: 0.05, false: 0.95 };
  return new Response(JSON.stringify({ operation: r.operation, answer: null, probabilities, confidence: 1, threshold_action: "REVIEW", extensions: { "x-decision_id": `dec-${r.question_id}` } }));
};

describe("decision client", () => {
  test("gateRequest carries motif, primitive and subject_ref", () => {
    const r = gateRequest({ producer: "p", decision_bank_id: "bank.x", question_id: "q", instructions: "i", primitive_id: "prim", subject_ref: "s", motif: "CONFIDENCE_GATE" });
    expect(r.operation).toBe("GATE");
    expect(r.extensions).toEqual({ "x-motif": "CONFIDENCE_GATE", "x-primitive_id": "prim", "x-subject_ref": "s" });
  });
  test("shadowGate posts backend + returns decision id; failures are null, never throw", async () => {
    const calls: Call[] = [];
    const g = await shadowGate({ producer: "p", decision_bank_id: "b", question_id: "q", instructions: "i" }, "state", { url: "http://s1", fetchImpl: fakeRuntime(0.9, calls), backend: "auto", enabled: true });
    expect(g?.decision_id).toBe("dec-q");
    expect(g?.recommendation).toBe("allow");
    expect(calls[0].body.backend).toBe("auto");
    expect(await decide({}, "s", { fetchImpl: async () => { throw new TypeError("down"); }, enabled: true })).toBeNull();
    expect(await reportOutcome({ truth: "true", source: "x" }, { enabled: true, fetchImpl: fakeRuntime(1, calls) })).toBe(false);
  });
});

describe("CLI guard hook: shadow GATE", () => {
  const input = (command: string) => ({ tool_name: "Bash", tool_input: { command }, cwd: "/tmp", tool_use_id: "toolu_1" });
  test("logs a GATE with subject_ref, records it, and leaves the emitted decision unchanged", async () => {
    for (const p of [0.99, 0.01]) {
      const calls: Call[] = [];
      const r = await runGuard(input("ls -la"), { mode: "advise", logDir: null, serverUrl: "http://s1", fetchImpl: fakeRuntime(0.0, calls) as any });
      const shadow = await runGuard(input("ls -la"), { mode: "advise", logDir: null, serverUrl: "http://s1", fetchImpl: fakeRuntime(p, []) as any });
      const gate = calls.find((c) => c.body.request.operation === "GATE")!;
      expect(gate.body.request.decision_bank_id).toBe(GUARD_GATE.bank);
      expect(gate.body.request.extensions["x-subject_ref"]).toBe("cc-tool:toolu_1");
      expect(r.record!.s1_gate!.decision_id).toBe(`dec-${GUARD_GATE.question}`);
      // incumbent unchanged regardless of what S1 says
      expect(shadow.output).toEqual(r.output);
    }
  });
  test("x-incumbent: auto-approve harness = true, other modes send none", async () => {
    const gateOf = async (inp: any, mode: "log" | "advise") => {
      const calls: Call[] = [];
      await runGuard(inp, { mode, logDir: null, serverUrl: "http://s1", fetchImpl: fakeRuntime(0.0, calls) as any });
      return calls.find((c) => c.body.request.operation === "GATE")!.body.request.extensions;
    };
    const yolo = { ...input("ls -la"), permission_mode: "bypassPermissions" };
    expect((await gateOf(yolo, "log"))["x-incumbent"]).toBe("true");
    expect((await gateOf(yolo, "advise"))["x-incumbent"]).toBe("true");
    expect((await gateOf({ ...input("ls -la"), permission_mode: "default" }, "log"))["x-incumbent"]).toBeUndefined();
    expect(harnessIncumbent({}, { SYSTEM_ONE_HARNESS_AUTO_APPROVE: "1" } as any)).toBe("allow");
    expect(harnessIncumbent({ permission_mode: "default" }, { SYSTEM_ONE_HARNESS_AUTO_APPROVE: "1" } as any)).toBeNull();
  });
  test("tightened view never emits allow over an ask, and hard-rule calls skip S1", async () => {
    const calls: Call[] = [];
    const hot = await runGuard(input("rm -rf ~"), { mode: "advise", logDir: null, serverUrl: "http://s1", fetchImpl: fakeRuntime(1, calls) as any });
    expect(hot.output?.hookSpecificOutput.permissionDecision).toBe("deny");
    expect(calls.length).toBe(0);
  });
  test("PostToolUse reports the 'proceeded' outcome label by subject_ref", async () => {
    const calls: Call[] = [];
    expect(await reportToolRan({ tool_use_id: "toolu_1" }, { serverUrl: "http://s1", fetchImpl: fakeRuntime(1, calls) as any })).toBe(true);
    expect(calls[0].url).toBe("http://s1/v1/decision/outcome");
    expect(calls[0].body).toEqual({ subject_ref: "cc-tool:toolu_1", truth: "true", source: "cli_hook.post_tool_use" });
    expect(await reportToolRan({}, { fetchImpl: fakeRuntime(1, calls) as any })).toBe(false);
  });
  test("PostToolUseFailure reports true: the call was allowed, it ran and failed", async () => {
    const calls: Call[] = [];
    await handleOutcomeHook({ hook_event_name: "PostToolUseFailure", tool_use_id: "toolu_2" }, { serverUrl: "http://s1", fetchImpl: fakeRuntime(1, calls) as any, pendingDir: null });
    expect(calls[0].body).toEqual({ subject_ref: "cc-tool:toolu_2", truth: "true", source: OUTCOME_SOURCES.ranFailed });
  });
  test("a PermissionRequest with no run before Stop is labelled denied; one that ran is not", async () => {
    const pendingDir = mkdtempSync(joinPath(tmpdir(), "s1-pending-"));
    const calls: Call[] = [];
    const deps = { serverUrl: "http://s1", fetchImpl: fakeRuntime(1, calls) as any, pendingDir };
    const call = (cmd: string, id: string) => ({ session_id: "sess-1", tool_name: "Bash", tool_input: { command: cmd }, tool_use_id: id, cwd: "/tmp" });
    // PreToolUse remembers input → id; PermissionRequest inputs carry no tool_use_id.
    await runGuard(call("git push", "toolu_a"), { mode: "log", logDir: null, pendingDir, serverUrl: "http://s1", fetchImpl: fakeRuntime(0.5, []) as any });
    await runGuard(call("npm publish", "toolu_b"), { mode: "log", logDir: null, pendingDir, serverUrl: "http://s1", fetchImpl: fakeRuntime(0.5, []) as any });
    const { tool_use_id: _a, ...askA } = call("git push", "toolu_a");
    const { tool_use_id: _b, ...askB } = call("npm publish", "toolu_b");
    await handleOutcomeHook({ hook_event_name: "PermissionRequest", ...askA }, deps);
    await handleOutcomeHook({ hook_event_name: "PermissionRequest", ...askB }, deps);
    // The person allowed toolu_a (it ran); toolu_b never ran.
    await handleOutcomeHook({ hook_event_name: "PostToolUse", ...call("git push", "toolu_a") }, deps);
    await handleOutcomeHook({ hook_event_name: "Stop", session_id: "sess-1" }, deps);
    expect(calls.map((c) => c.body)).toEqual([
      { subject_ref: "cc-tool:toolu_a", truth: "true", source: OUTCOME_SOURCES.ran },
      { subject_ref: "cc-tool:toolu_b", truth: "false", source: OUTCOME_SOURCES.denied },
    ]);
    // The session's pending files are gone; a second Stop labels nothing.
    expect(existsSync(joinPath(pendingDir, "sess-1"))).toBe(false);
    await handleOutcomeHook({ hook_event_name: "Stop", session_id: "sess-1" }, deps);
    expect(calls.length).toBe(2);
  });
});
