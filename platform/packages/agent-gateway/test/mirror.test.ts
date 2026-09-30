import { describe, expect, it } from "vitest";
import { BaseAaiProvider, computeMirrorState, MirrorSync, ok, fail, planMirrorWrites, type MirrorFieldSpec, type AaiResult, type SnapshotResult } from "../src/index.js";
import { mirrorFieldStateSchema } from "@allternit/subscription-fabric-contracts";

const specs: MirrorFieldSpec[] = [
  { field: "name", observability: "exact", writable: true },
  { field: "persona", observability: "partial", writable: true },
  { field: "avatar", observability: "exact", writable: false },
  { field: "memory", observability: "none", writable: false },
];
const st = (r: { field: string; state: string }[], f: string) => r.find((x) => x.field === f)?.state;

describe("computeMirrorState", () => {
  it("in_sync after normalization (whitespace, key order)", () => {
    const r = computeMirrorState({ name: "Ada  Lovelace ", persona: { b: 1, a: [" x "] } }, { values: { name: "Ada Lovelace", persona: { a: ["x"], b: 1 } } }, specs);
    expect(st(r, "name")).toBe("in_sync");
    expect(st(r, "persona")).toBe("in_sync");
    for (const f of r) expect(mirrorFieldStateSchema.safeParse(f).success).toBe(true);
  });
  it("unobservable (none, or missing from snapshot) is never claimed synced, even when local matches", () => {
    const r = computeMirrorState({ name: "A" }, { values: { name: "A", memory: "m", persona: "p", avatar: "a" } }, specs);
    expect(st(r, "memory")).toBe("unsupported");
    expect(st(r, "persona")).toBe("unsupported"); // partial but absent from this snapshot
    expect(st(r, "avatar")).toBe("unsupported");
    expect(r.find((f) => f.field === "memory")?.remoteValueRef).toBeUndefined();
  });
  it("remote_ahead when only remote moved from base; local_ahead when only local moved; conflict when both", () => {
    const base = { name: "v1", persona: "v1", avatar: "v1" };
    const r = computeMirrorState({ name: "v2", persona: "v1", avatar: "v2" }, { values: { name: "v1", persona: "v2", avatar: "v3" }, base }, specs);
    expect(st(r, "name")).toBe("remote_ahead");
    expect(st(r, "persona")).toBe("local_ahead");
    expect(st(r, "avatar")).toBe("conflict");
  });
  it("differing with no base is a conflict; nothing local is remote_ahead", () => {
    expect(st(computeMirrorState({ name: "x" }, { values: { name: "y" } }, specs), "name")).toBe("conflict");
    expect(st(computeMirrorState({ name: "x" }, { values: {} }, specs), "name")).toBe("remote_ahead");
  });
  it("states carry opaque refs, never raw values", () => {
    const r = computeMirrorState({ name: "secret-remote" }, { values: { name: "secret-local" } }, specs);
    expect(JSON.stringify(r)).not.toMatch(/secret/);
    expect(r[0].localValueRef).toMatch(/^fnv1a:/);
  });
});

describe("planMirrorWrites", () => {
  it("refuses non-writable and undeclared fields", () => {
    const p = planMirrorWrites({ name: "n", avatar: "a", memory: "m", ghost: 1 }, specs);
    expect(p.writes).toEqual([{ field: "name", value: "n" }]);
    expect(p.refused).toEqual([{ field: "avatar", reason: "not_writable" }, { field: "memory", reason: "not_writable" }, { field: "ghost", reason: "unknown_field" }]);
  });
});

class SnapProvider extends BaseAaiProvider {
  readonly adapterId = "snap"; calls = 0;
  constructor(public fields: Record<string, unknown>, public err = false) { super(); }
  async snapshot(): Promise<AaiResult<SnapshotResult>> {
    this.calls++;
    return this.err ? fail("VENDOR_UNAVAILABLE", "down") : ok({ agentId: "a", fields: { ...this.fields } });
  }
}

describe("MirrorSync", () => {
  it("reports states, honest partial sync, and detects remote drift between snapshots", async () => {
    const p = new SnapProvider({ name: "A", avatar: "x" });
    const m = new MirrorSync(p, "a", specs, () => "T");
    const r1 = await m.sync({ name: "A", avatar: "x", memory: "m" });
    expect(r1.ok && r1.value.fullySynced).toBe(false); // memory + persona unobservable
    expect(r1.ok && r1.value.unobservable.sort()).toEqual(["memory", "persona"]);
    expect(r1.ok && r1.value.remoteDrift).toEqual([]);
    p.fields.name = "B";
    const r2 = await m.sync({ name: "A", avatar: "x" });
    expect(r2.ok && r2.value.remoteDrift).toEqual(["name"]);
    expect(r2.ok && r2.value.fields.find((f) => f.field === "name")?.state).toBe("remote_ahead"); // base learned from r1
    p.fields.name = "C";
    const r3 = await m.sync({ name: "A2", avatar: "x" }); // local also moved from base A
    expect(r3.ok && r3.value.fields.find((f) => f.field === "name")?.state).toBe("conflict");
  });
  it("fullySynced only when every declared field is observable and equal", async () => {
    const one: MirrorFieldSpec[] = [{ field: "name", observability: "exact", writable: true }];
    const r = await new MirrorSync(new SnapProvider({ name: "A" }), "a", one).sync({ name: "A" });
    expect(r.ok && r.value.fullySynced).toBe(true);
  });
  it("passes provider errors through and refuses writes via plan()", async () => {
    const m = new MirrorSync(new SnapProvider({}, true), "a", specs);
    const r = await m.sync({});
    expect(!r.ok && r.error.code).toBe("VENDOR_UNAVAILABLE");
    expect(m.plan({ avatar: 1 }).refused).toHaveLength(1);
  });
  it("UNSUPPORTED snapshot surfaces as UNSUPPORTED (adapter has no snapshot)", async () => {
    const r = await new MirrorSync(new (class extends BaseAaiProvider { readonly adapterId = "x"; })(), "a", specs).sync({});
    expect(!r.ok && r.error.code).toBe("UNSUPPORTED");
  });
});
