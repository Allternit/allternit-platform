// Per-component state machine. Pure: no I/O, no clock reads.
//
// Rules (plan, "Status logic"):
//  Down      a critical target fails 3 Cloudflare ticks AND the last 2 standby reports.
//            If the standby vantage is stale (> 3 min), 5 Cloudflare ticks instead.
//  Degraded  one vantage fails 3 ticks; a non-critical target fails; a "ready" target
//            is down while a "live" target of the same component passes; or the
//            component's median latency is over 3 s for 5 ticks.
//  Recovered 3 passes in a row on both vantages (Cloudflare only while standby is stale).
//  max_status caps a component (Other sites never go past Degraded).
import type { ProbeResult, Status, Target, TargetComponent, TargetsDoc } from "../contract/types";

export const CF_FAIL_DOWN = 3;
export const CF_FAIL_DOWN_STALE = 5;
export const SB_FAIL_DOWN = 2;
export const ONE_VANTAGE_FAIL = 3;
export const RECOVER_PASSES = 3;
export const STANDBY_STALE_MS = 3 * 60_000;
/** Standby reports older than this are ignored for streaks. */
export const STANDBY_WINDOW_MS = 6 * 60_000;
export const SLOW_MS = 3000;
export const SLOW_TICKS = 5;

export type MachineStatus = "operational" | "degraded" | "down" | "unknown";

export interface TargetMem {
  /** Consecutive Cloudflare failures (saturates at CF_FAIL_DOWN_STALE). */
  f: number;
  /** Consecutive Cloudflare passes (saturates at RECOVER_PASSES). */
  p: number;
}

export interface Machine {
  t: Record<string, TargetMem>;
  /** Consecutive ticks with median latency over SLOW_MS (saturates at SLOW_TICKS). */
  slow: number;
  /** Consecutive ticks with median latency at or under SLOW_MS (saturates at RECOVER_PASSES). */
  fast: number;
  /** Consecutive ticks where the candidate status was better than the current one. */
  imp: number;
}

export interface ComponentRecord {
  status: MachineStatus;
  last_change: string | null;
  machine: Machine;
}

export interface StandbyReport {
  ts: string;
  results: ProbeResult[];
}

export interface StepInput {
  now: Date;
  targets: TargetsDoc;
  /** This tick's Cloudflare results. Targets not probed this tick keep their streaks. */
  cf: ProbeResult[];
  /** Standby reports, newest first. */
  standby: StandbyReport[];
  prev: Map<string, ComponentRecord>;
}

export interface Transition {
  component_id: string;
  from: MachineStatus;
  to: MachineStatus;
}

export interface StepOutput {
  next: Map<string, ComponentRecord>;
  transitions: Transition[];
  standbyFresh: boolean;
}

const RANK: Record<MachineStatus, number> = { unknown: -1, operational: 0, degraded: 1, down: 2 };
const worse = (a: MachineStatus, b: MachineStatus): MachineStatus => (RANK[a] >= RANK[b] ? a : b);

export function emptyMachine(): Machine {
  return { t: {}, slow: 0, fast: 0, imp: 0 };
}

function median(xs: number[]): number {
  const s = [...xs].sort((a, b) => a - b);
  const m = s.length >> 1;
  return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2;
}

export function step(input: StepInput): StepOutput {
  const nowMs = input.now.getTime();
  const nowIso = input.now.toISOString();
  const reports = input.standby.filter((r) => {
    const age = nowMs - Date.parse(r.ts);
    return Number.isFinite(age) && age <= STANDBY_WINDOW_MS;
  });
  const standbyFresh = reports.length > 0 && nowMs - Date.parse(reports[0].ts) <= STANDBY_STALE_MS;
  const cfById = new Map(input.cf.map((r) => [r.target_id, r]));

  const sbFind = (r: StandbyReport, id: string) => r.results.find((x) => x.target_id === id);
  const sbAll = (id: string, n: number, ok: boolean) =>
    reports.length >= n && reports.slice(0, n).every((r) => sbFind(r, id)?.ok === ok);

  const next = new Map<string, ComponentRecord>();
  const transitions: Transition[] = [];

  for (const comp of input.targets.components) {
    const prev = input.prev.get(comp.id);
    const cur: MachineStatus = prev?.status ?? "unknown";
    const m: Machine = prev ? structuredClone(prev.machine) : emptyMachine();
    const mine = input.targets.targets.filter((t) => t.component_id === comp.id);

    if (mine.length === 0) {
      // No check yet (cloud computers until A7). Stays unknown.
      const rec = { status: "unknown" as const, last_change: prev?.last_change ?? null, machine: m };
      if (cur !== "unknown") transitions.push({ component_id: comp.id, from: cur, to: "unknown" });
      next.set(comp.id, rec);
      continue;
    }

    const levels = new Map<string, MachineStatus>();
    const latencies: number[] = [];
    let clean = true;
    let anyData = false;

    for (const t of mine) {
      const r = cfById.get(t.id);
      const mem = (m.t[t.id] ??= { f: 0, p: 0 });
      if (r) {
        if (r.ok) {
          mem.p = Math.min(mem.p + 1, RECOVER_PASSES);
          mem.f = 0;
          if (typeof r.latency_ms === "number") latencies.push(r.latency_ms);
        } else {
          mem.f = Math.min(mem.f + 1, CF_FAIL_DOWN_STALE);
          mem.p = 0;
        }
      }
      if (!r && mem.f === 0 && mem.p === 0) continue; // never probed: no data
      anyData = true;
      levels.set(t.id, targetLevel(t, comp, mem, standbyFresh, reports, sbAll));
      const sbUsed = standbyFresh && t.vantages.includes("allternit-standby") && !!sbFind(reports[0], t.id);
      if (!(mem.p >= RECOVER_PASSES && (!sbUsed || sbAll(t.id, RECOVER_PASSES, true)))) clean = false;
    }

    // "ready" down while "live" passes: the service answers, a dependency is failing.
    for (const t of mine) {
      if (t.role === "ready" && levels.get(t.id) === "down") {
        const livePasses = mine.some((o) => o.role === "live" && o.id !== t.id && levels.get(o.id) === "operational");
        if (livePasses) levels.set(t.id, "degraded");
      }
    }

    if (latencies.length) {
      if (median(latencies) > SLOW_MS) {
        m.slow = Math.min(m.slow + 1, SLOW_TICKS);
        m.fast = 0;
      } else {
        m.fast = Math.min(m.fast + 1, RECOVER_PASSES);
        m.slow = 0;
      }
    }
    if (m.slow > 0 || m.fast < RECOVER_PASSES) clean = false;

    let candidate: MachineStatus = anyData ? "operational" : "unknown";
    for (const l of levels.values()) candidate = worse(candidate, l);
    if (m.slow >= SLOW_TICKS) candidate = worse(candidate, "degraded");
    if (comp.max_status === "degraded" && candidate === "down") candidate = "degraded";

    let status = cur;
    if (cur === "unknown" || RANK[candidate] > RANK[cur]) {
      status = candidate;
      m.imp = 0;
    } else if (RANK[candidate] === RANK[cur]) {
      m.imp = 0;
    } else {
      m.imp = Math.min(m.imp + 1, RECOVER_PASSES);
      const ready = candidate === "operational" ? clean : m.imp >= RECOVER_PASSES;
      if (ready) {
        status = candidate;
        m.imp = 0;
      }
    }

    const changed = status !== cur;
    if (changed && cur !== "unknown") transitions.push({ component_id: comp.id, from: cur, to: status });
    next.set(comp.id, {
      status,
      last_change: changed && cur !== "unknown" ? nowIso : prev?.last_change ?? null,
      machine: m,
    });
  }

  return { next, transitions, standbyFresh };
}

function targetLevel(
  t: Target,
  comp: TargetComponent,
  mem: TargetMem,
  standbyFresh: boolean,
  reports: StandbyReport[],
  sbAll: (id: string, n: number, ok: boolean) => boolean,
): MachineStatus {
  const sbUsed =
    standbyFresh && t.vantages.includes("allternit-standby") && reports[0].results.some((x) => x.target_id === t.id);
  const hardDown = sbUsed ? mem.f >= CF_FAIL_DOWN && sbAll(t.id, SB_FAIL_DOWN, false) : mem.f >= CF_FAIL_DOWN_STALE;
  const oneVantage = mem.f >= ONE_VANTAGE_FAIL || (sbUsed && sbAll(t.id, ONE_VANTAGE_FAIL, false));
  if (hardDown) return t.critical && comp.critical ? "down" : "degraded";
  if (oneVantage) return "degraded";
  return "operational";
}

/** Overall page status: the worst component, ignoring unknown. */
export function overallOf(statuses: Status[]): Status {
  const rank: Record<Status, number> = { unknown: -1, operational: 0, maintenance: 1, degraded: 2, down: 3 };
  let best: Status = "unknown";
  for (const s of statuses) if (rank[s] > rank[best]) best = s;
  return best;
}
