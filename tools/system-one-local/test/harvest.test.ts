import { describe, expect, test } from "bun:test";
import { Database } from "bun:sqlite";
import { mkdtempSync, readFileSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { DecisionRequestV1 } from "../src/decision/contract.ts";
import { buildExport } from "../src/decision/export.ts";
import { DecisionRouter } from "../src/decision/router.ts";
import type { DecisionReadoutProvider, Readout } from "../src/decision/readout.ts";
import { optionsOf } from "../src/decision/readout.ts";
import { harvestDecisionId, redactState, runHarvest } from "../src/harvest/run.ts";
import { BANKS } from "../src/harvest/banks.ts";
import { genClassOf, routeLabel, routeModelLabel } from "../src/harvest/labels.ts";
import { itemsFromTranscript } from "../src/harvest/sources/claude-code.ts";
import { gizziSource } from "../src/harvest/sources/gizzi.ts";
import { ShadowLedger } from "../src/decision/shadow.ts";
import type { HarvestItem, HarvestSource } from "../src/harvest/types.ts";

class FakeProvider implements DecisionReadoutProvider {
  readonly backend_id = "backend.fake";
  calls = 0;
  async readout(req: DecisionRequestV1, _state: string): Promise<Readout> {
    this.calls++;
    const options = optionsOf(req);
    return { options, probs: options.map(() => 1 / options.length), method: "fake", kind: "logprob", shape: "categorical", deployment: { backend_id: this.backend_id, model_ref: "fake", model_revision: "r1", tokenizer_id: "t", quantization: "none", runtime_backend: "test" } } as Readout;
  }
}

const L = (o: unknown) => JSON.stringify(o);
const transcript = [
  L({ type: "user", uuid: "u1", sessionId: "s1", timestamp: "2026-09-01T10:00:00Z", permissionMode: "bypassPermissions", message: { role: "user", content: "fix the failing test in foo.ts, key sk-ant-abcdefghijklmnopqrstuv" } }),
  L({ type: "assistant", timestamp: "2026-09-01T10:00:01Z", message: { model: "claude-opus-5-5", content: [{ type: "tool_use", id: "t1", name: "Read", input: { file_path: "/x/foo.ts" } }, { type: "tool_use", id: "t2", name: "Edit", input: { file_path: "/x/foo.ts" } }] } }),
  L({ type: "user", timestamp: "2026-09-01T10:00:02Z", message: { role: "user", content: [{ type: "tool_result", tool_use_id: "t1", content: "ok" }, { type: "tool_result", tool_use_id: "t2", content: "ok" }] } }),
  L({ type: "user", uuid: "u2", sessionId: "s1", timestamp: "2026-09-01T10:01:00Z", permissionMode: "default", message: { role: "user", content: "what is 2+2" } }),
  L({ type: "assistant", timestamp: "2026-09-01T10:01:01Z", message: { model: "claude-opus-5-5", content: [{ type: "tool_use", id: "t3", name: "Bash", input: { command: "rm -rf /tmp/x" } }] } }),
  L({ type: "user", timestamp: "2026-09-01T10:01:02Z", toolDenialKind: "user-rejected", message: { role: "user", content: [{ type: "tool_result", tool_use_id: "t3", is_error: true, content: "rejected" }] } }),
  L({ type: "user", uuid: "u3", sessionId: "s1", timestamp: "2026-09-01T10:02:00Z", message: { role: "user", content: "<command-name>/clear</command-name>" } }),
  L({ type: "user", uuid: "u4", sessionId: "s1", isSidechain: true, timestamp: "2026-09-01T10:03:00Z", message: { role: "user", content: "agent-written prompt" } }),
];

describe("label rules (ported from turn-router.ts)", () => {
  test("routeLabel", () => {
    expect(routeLabel([])).toBe("answer_from_memory");
    expect(routeLabel(["Read", "Edit"])).toBe("coding");
    expect(routeLabel(["Bash"])).toBe("single_tool");
    expect(routeLabel(["Read", "Grep"])).toBe("retrieval");
    expect(routeLabel(["AskUserQuestion"])).toBe("clarify");
    expect(routeLabel(["Agent"])).toBe("agent_run");
  });
  test("routeModelLabel follows the person's next move", () => {
    const cur = { text: "a", cls: "gen.standard" as const, errored: false, requested: "m" };
    expect(routeModelLabel(cur, { text: "b", cls: "gen.standard", requested: "m" })).toEqual({ truth: "gen.standard", source: "turn_accepted" });
    expect(routeModelLabel(cur, { text: "a", cls: "gen.standard", requested: "m" })).toEqual({ truth: "gen.deep", source: "user_retry" });
    expect(routeModelLabel(cur, { text: "b", cls: "gen.small", requested: "n" })).toEqual({ truth: "gen.small", source: "user_model_switch" });
    expect(routeModelLabel({ ...cur, errored: true }, null).source).toBe("turn_error");
    expect(genClassOf("claude-opus-5-5")).toBe("gen.deep");
    expect(genClassOf("<synthetic>")).toBeNull();
  });
});

describe("claude-code parser", () => {
  const items = itemsFromTranscript(transcript, "f");
  test("turns -> ROUTE / ROUTE_MODEL with backfill provenance; commands and sidechains skipped", () => {
    const route = items.filter((i) => i.spec === BANKS.route);
    expect(route.map((i) => i.truth)).toEqual(["coding", "single_tool"]);
    expect(route.every((i) => i.label_source === "backfill_observed")).toBe(true);
    const rm = items.filter((i) => i.spec === BANKS.route_model);
    expect(rm.map((i) => [i.truth, i.outcome_source])).toEqual([["gen.deep", "replay:claude-code.turn_accepted"], ["gen.deep", "replay:claude-code.session_end"]]);
  });
  test("permission: human rejection observed false, bypass run replayed true", () => {
    const p = items.filter((i) => i.spec === BANKS.permission_judge);
    const byId = Object.fromEntries(p.map((i) => [i.key.split(":")[1], i]));
    expect(byId.t3.truth).toBe("false");
    expect(byId.t3.label_source).toBe("observed");
    expect(byId.t3.state).toBe("tool: Bash\ncommand: rm -rf /tmp/x\npaths: ");
    expect(byId.t1.truth).toBe("true");
    expect(byId.t1.label_source).toBe("backfill_observed");
    expect(byId.t1.capGroup).toBe("bypass");
    expect(items.filter((i) => i.spec === BANKS.permission_cli_guard).length).toBe(p.length);
  });
});

describe("gizzi parser", () => {
  test("reads message/part rows read-only", () => {
    const dir = mkdtempSync(join(tmpdir(), "gz-"));
    const db = new Database(join(dir, "g.db"));
    db.run("create table message (id text, session_id text, time_created integer, time_updated integer, data text)");
    db.run("create table part (id text, message_id text, session_id text, time_created integer, time_updated integer, data text)");
    db.run("insert into message values ('m1','s',1,1,?)", [L({ role: "user", model: { providerID: "p", modelID: "kimi-k3" } })]);
    db.run("insert into part values ('p1','m1','s',1,1,?)", [L({ type: "text", text: "search the docs" })]);
    db.run("insert into message values ('m2','s',2,2,?)", [L({ role: "assistant", modelID: "kimi-k3" })]);
    db.run("insert into part values ('p2','m2','s',2,2,?)", [L({ type: "tool", tool: "grep" })]);
    db.close();
    const items = [...(gizziSource(join(dir, "g.db")).items() as Iterable<HarvestItem>)];
    expect(items.find((i) => i.spec === BANKS.route)!.truth).toBe("retrieval");
    expect(items.find((i) => i.spec === BANKS.route_model)!.truth).toBe("gen.standard");
  });
});

describe("runner", () => {
  test("redaction strips secrets and emails before replay/storage", () => {
    const out = redactState("token sk-ant-abcdefghijklmnopqrstuv mail a@b.com");
    expect(out).not.toContain("sk-ant-abcdefghijklmnopqrstuv");
    expect(out).not.toContain("a@b.com");
  });

  test("replays through the router, stores state + provenance, and is idempotent", async () => {
    const dir = mkdtempSync(join(tmpdir(), "hv-"));
    const fake = new FakeProvider();
    const router = new DecisionRouter({ provider: fake, manifests: [], ledger: new ShadowLedger(dir) });
    const src: HarvestSource = { name: "t", items: () => itemsFromTranscript(transcript, "f") };
    const a = await runHarvest({ dir, router, sources: [src] });
    expect(a.failed).toBe(0);
    expect(a.replayed).toBeGreaterThan(0);
    const decs = readdirSync(join(dir, "decisions")).flatMap((f) => readFileSync(join(dir, "decisions", f), "utf8").trim().split("\n").map((l) => JSON.parse(l)));
    expect(decs.length).toBe(a.replayed);
    expect(decs.every((d) => d.decision_id.startsWith("h-") && d.ts.startsWith("2026-09-01") && typeof d.state === "string")).toBe(true);
    expect(decs.some((d) => d.state.includes("sk-ant-abcdefghijklmnopqrstuv"))).toBe(false);
    const b = await runHarvest({ dir, router, sources: [src] });
    expect(b.replayed).toBe(0);
    expect(b.skipped_existing).toBe(a.replayed);
    expect(fake.calls).toBe(a.replayed);
    // export joins it with label provenance
    const { rows } = buildExport(join(dir, "nonexistent-live"), { extraDirs: [dir] });
    expect(rows.length).toBeGreaterThan(0);
    expect(rows.every((r) => r.provenance.label_source !== undefined)).toBe(true);
  });

  test("stable decision ids", () => {
    const it = { spec: BANKS.route, source: "claude-code", key: "s:u" };
    expect(harvestDecisionId(it)).toBe(harvestDecisionId({ ...it }));
    expect(harvestDecisionId(it)).not.toBe(harvestDecisionId({ ...it, key: "s:v" }));
  });

  test("teacher labels are training rows only (Q26)", async () => {
    const dir = mkdtempSync(join(tmpdir(), "hv-"));
    const router = new DecisionRouter({ provider: new FakeProvider(), manifests: [], ledger: new ShadowLedger(dir) });
    const items: HarvestItem[] = Array.from({ length: 60 }, (_, i) => ({ spec: BANKS.route, key: `k${i}`, source: "t", ts: `2026-09-0${1 + (i % 9)}T00:00:00Z`, state: `turn ${i}`, truth: "coding", label_source: i % 2 ? "teacher" : "backfill_observed", outcome_source: i % 2 ? "teacher:test" : "replay:test" }));
    await runHarvest({ dir, router, sources: [{ name: "t", items: () => items }] });
    const { rows, summary } = buildExport(join(dir, "none"), { extraDirs: [dir] });
    const teacher = rows.filter((r) => r.provenance.label_source === "teacher");
    expect(teacher.length).toBe(30);
    expect(teacher.every((r) => r.split === "train")).toBe(true);
    expect(summary.groups[0].by_label_source.teacher).toBe(30);
    expect(summary.groups[0].meets_q26_minimums).toBe(false);
  });
});
