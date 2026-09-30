// Mirror field-level sync engine (spec: agent-gateway.md "Mirror rules").
//  - Writes only for fields the adapter can write back; otherwise local editing is disabled (planMirrorWrites refuses).
//  - A field the adapter cannot observe is never claimed synced (status "unobservable").
//  - Drift is computed on NORMALIZED snapshots (agent.snapshot fields), never raw DOM.
// States follow the spec: synced | partial | stale (direction remote_ahead|local_ahead) | conflict | unobservable.
import type { AaiProvider, AaiResult, MirrorFieldState } from "./types.js";
import { ok } from "./types.js";

export type MirrorObservability = "exact" | "partial" | "none";

/** Declared by the adapter: which snapshot fields exist, how well they are observed, and whether they can be written back. */
export interface MirrorFieldSpec {
  field: string;
  observability: MirrorObservability;
  writable: boolean;
  /** Canonicalises a value before comparison. Default: `normalizeValue`. */
  normalize?: (v: unknown) => unknown;
}

/** Local copy of the mirrored fields plus the last value both sides agreed on (per field), if any. */
export interface LocalMirror {
  values: Record<string, unknown>;
  base?: Record<string, unknown>;
}

/** Default canonicalisation: trim + collapse whitespace in strings, sort object keys, recurse arrays. */
export function normalizeValue(v: unknown): unknown {
  if (typeof v === "string") return v.replace(/\s+/g, " ").trim();
  if (Array.isArray(v)) return v.map(normalizeValue);
  if (v && typeof v === "object") {
    return Object.fromEntries(Object.keys(v as object).sort().map((k) => [k, normalizeValue((v as Record<string, unknown>)[k])]));
  }
  return v;
}

const canon = (v: unknown): string => JSON.stringify(v === undefined ? null : v);
/** Short opaque reference (FNV-1a) so states never carry raw values. */
function ref(canonical: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < canonical.length; i++) { h ^= canonical.charCodeAt(i); h = Math.imul(h, 0x01000193) >>> 0; }
  return `fnv1a:${h.toString(16).padStart(8, "0")}`;
}
const has = (o: Record<string, unknown> | undefined, k: string) => !!o && Object.prototype.hasOwnProperty.call(o, k) && o[k] !== undefined;

export function computeMirrorState(
  remoteSnapshot: Record<string, unknown>,
  local: LocalMirror,
  fieldSpecs: MirrorFieldSpec[],
  now: () => string = () => new Date().toISOString(),
): MirrorFieldState[] {
  const checkedAt = now();
  return fieldSpecs.map((spec): MirrorFieldState => {
    const norm = spec.normalize ?? normalizeValue;
    const localHas = has(local.values, spec.field);
    const lRef = localHas ? ref(canon(norm(local.values[spec.field]))) : undefined;
    const obs = spec.observability;
    const withRefs = (status: MirrorFieldState["status"], r?: string, direction?: MirrorFieldState["direction"]): MirrorFieldState => ({
      field: spec.field, authority: "vendor", observability: obs,
      // In sync on a partially observable field is only `partial`, never `synced`.
      status: status === "synced" && obs === "partial" ? "partial" : status,
      ...(direction ? { direction } : {}),
      ...(lRef ? { localVersion: lRef } : {}), ...(r ? { remoteVersion: r } : {}), checkedAt,
    });
    // Unobservable: no vendor read access, or the snapshot did not carry the field. Never claimed synced.
    if (spec.observability === "none" || !has(remoteSnapshot, spec.field)) return withRefs("unobservable");
    const rc = canon(norm(remoteSnapshot[spec.field])), rRef = ref(rc);
    const lc = localHas ? canon(norm(local.values[spec.field])) : undefined;
    if (lc === rc) return withRefs("synced", rRef);
    if (lc === undefined) return withRefs("stale", rRef, "remote_ahead"); // nothing local yet: local is behind
    if (!has(local.base, spec.field)) return withRefs("conflict", rRef); // differ and no common base: direction unknown
    const bc = canon(norm(local.base![spec.field]));
    if (lc === bc) return withRefs("stale", rRef, "remote_ahead"); // only remote moved
    if (rc === bc) return withRefs("stale", rRef, "local_ahead"); // only local moved
    return withRefs("conflict", rRef); // both moved
  });
}

export interface PlannedWrite { field: string; value: unknown }
export interface RefusedWrite { field: string; reason: "unknown_field" | "not_writable" }
export interface MirrorWritePlan { writes: PlannedWrite[]; refused: RefusedWrite[] }

/** Local edits to non-writable (or undeclared) fields are refused; the UI should disable editing them. */
export function planMirrorWrites(localEdits: Record<string, unknown>, fieldSpecs: MirrorFieldSpec[]): MirrorWritePlan {
  const by = new Map(fieldSpecs.map((s) => [s.field, s]));
  const plan: MirrorWritePlan = { writes: [], refused: [] };
  for (const [field, value] of Object.entries(localEdits)) {
    const s = by.get(field);
    if (!s) plan.refused.push({ field, reason: "unknown_field" });
    else if (!s.writable) plan.refused.push({ field, reason: "not_writable" });
    else plan.writes.push({ field, value });
  }
  return plan;
}

export interface MirrorReport {
  fields: MirrorFieldState[];
  /** Fields whose normalized remote value changed since the previous sync() (undefined on the first run). */
  remoteDrift: string[];
  /** true when every declared field is `synced` (partial fields can never make this true) */
  fullySynced: boolean;
  /** Fields that cannot be claimed synced (unobservable). Shown honestly as partial sync. */
  unobservable: string[];
}

/** Pulls normalized snapshots via `agent.snapshot` and reports per-field state. Never touches raw DOM. */
export class MirrorSync {
  private last: Record<string, string> | undefined;
  private base: Record<string, unknown> = {};
  constructor(private provider: AaiProvider, private agentId: string, private specs: MirrorFieldSpec[], private now?: () => string) {}

  async sync(localValues: Record<string, unknown>): Promise<AaiResult<MirrorReport>> {
    const snap = await this.provider.snapshot({ agentId: this.agentId });
    if (!snap.ok) return snap;
    const remote = snap.value.fields;
    const fields = computeMirrorState(remote, { values: localValues, base: this.base }, this.specs, this.now);
    const cur: Record<string, string> = {};
    for (const s of this.specs) {
      if (s.observability !== "none" && has(remote, s.field)) cur[s.field] = canon((s.normalize ?? normalizeValue)(remote[s.field]));
    }
    const remoteDrift = this.last ? Object.keys({ ...this.last, ...cur }).filter((k) => this.last![k] !== cur[k]) : [];
    this.last = cur;
    // Record the agreed value as the new base only for fields that are provably in sync.
    for (const f of fields) if (f.status === "synced" || f.status === "partial") this.base[f.field] = remote[f.field];
    return ok({
      fields, remoteDrift,
      fullySynced: fields.length > 0 && fields.every((f) => f.status === "synced"),
      unobservable: fields.filter((f) => f.status === "unobservable").map((f) => f.field),
    });
  }

  plan(localEdits: Record<string, unknown>): MirrorWritePlan { return planMirrorWrites(localEdits, this.specs); }
}
