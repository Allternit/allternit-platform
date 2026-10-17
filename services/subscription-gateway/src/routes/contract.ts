// §3 "One contract, no fall-off": every route offered for a vendor must pass
// ALL of the capability-contract items, proven by the /aai/conformance suite
// (extended per route: the item results are recorded per
// (vendor, route, appVersion) — see routes/store.ts). A route missing even one
// item is not eligible and is not offered; it never quietly does less.
import type { ConformanceReport } from "@allternit/agent-gateway";

// The §3 checklist, one entry per bullet. Ids are stable; the Settings UI and
// the conformance store key off them.
export const CONTRACT_ITEMS = [
  {
    id: "chat.start_continue",
    text: "start a chat, continue a chat, pick the model/mode the app offers",
  },
  {
    id: "reply.stream_read",
    text: "read the reply as it streams; know when it's finished (not \"the screen stopped changing\")",
  },
  {
    id: "reply.stop_midstream",
    text: "stop a reply mid-stream",
  },
  {
    id: "attachments.send",
    text: "send attachments (image, file)",
  },
  {
    id: "outputs.exact_and_files",
    text: "get outputs back: text exact (not OCR guesses where a tree exists), generated images/files",
  },
  {
    id: "threads.list_reopen",
    text: "list and reopen past threads",
  },
  {
    id: "errors.typed_states",
    text: "report usage-limit, sign-out and verification-wall states as typed errors",
  },
] as const;
export type ContractItemId = (typeof CONTRACT_ITEMS)[number]["id"];
export const CONTRACT_ITEM_IDS = CONTRACT_ITEMS.map((i) => i.id) as ContractItemId[];

export interface ContractItemResult {
  item: ContractItemId;
  passed: boolean;
  /** Why it failed (or what provenance it has). Empty when passed. */
  reason: string;
  /** The conformance checks this verdict stands on (evidence, no secrets). */
  evidence: string[];
}

export interface ContractResult {
  /** appVersion the result was checked against; null = not checked at all. */
  appVersion: string | null;
  checkedAt: string | null;
  items: ContractItemResult[];
  passed: ContractItemId[];
  failed: ContractItemId[];
  /** Every item passed → the contract half of eligibility is satisfied. */
  ok: boolean;
}

// How each item is proven from a ConformanceReport. An item passes only when
// every listed check passed; a check the suite did not run is a failure for
// the item (no assumed passes — spec §3 "never assume", task item 7).
interface ItemRule {
  item: ContractItemId;
  /** [area, checkName] pairs that must all have status "pass". */
  checks: Array<[string, string]>;
  /** Extra report-wide condition (e.g. every failure-area mapping ok). */
}

// Check names below are the exact strings runConformance records (see
// @allternit/agent-gateway conformance.ts): on success the `safe()`-wrapped
// calls add no check, so e.g. the context area's pass entries are "send" and
// "terminate: close". A skipped fault check is recorded under the bare kind
// name ("vendor_down"), which the matcher reports as "check not run" — the
// honest not-proven direction, never an assumed pass.
const ITEM_RULES: ItemRule[] = [
  // start/continue a chat: the context area sends and closes cleanly.
  { item: "chat.start_continue", checks: [["context", "send"], ["context", "terminate: close"]] },
  // streaming read + finished detection: the events area is green (schema,
  // cursors, and the correlationId link between send and completion).
  { item: "reply.stream_read", checks: [["events", "events schema-valid"], ["events", "events have cursors"], ["events", "events reflect sent message (correlationId)"]] },
  // stop mid-stream: the cancellation area confirms the stop.
  { item: "reply.stop_midstream", checks: [["cancellation", "cancel"]] },
  // Attachments: the AAI conformance suite has no attachment check yet, so
  // this item can only be recorded via a route-level extension the adapter
  // ships; until then it is an explicit, visible failure.
  { item: "attachments.send", checks: [] },
  // Text exact + files out: idempotency proves exact reply replay, resources
  // proves artifact reads.
  { item: "outputs.exact_and_files", checks: [["idempotency", "replay returns the first result"], ["resources", "agent.artifacts"]] },
  // list/reopen past threads: the tasks/resources surface enumerates them.
  { item: "threads.list_reopen", checks: [["resources", "agent.tasks"]] },
  // typed error states: the failure area maps vendor_down / rate_limited /
  // auth_revoked / account_banned / ui_changed to typed codes.
  { item: "errors.typed_states", checks: [["failure", "vendor_down: maps to VENDOR_UNAVAILABLE"], ["failure", "rate_limited: maps to RATE_LIMITED"], ["failure", "auth_revoked: maps to AUTH_REVOKED|AUTH_REQUIRED"], ["failure", "account_banned: maps to LANE_BLOCKED|POLICY_DENIED"]] },
];

const NO_EVIDENCE: Record<ContractItemId, string> = {
  "chat.start_continue": "no conformance evidence",
  "reply.stream_read": "no conformance evidence",
  "reply.stop_midstream": "no conformance evidence",
  "attachments.send": "attachments are not covered by the AAI conformance suite yet; not proven, so not eligible",
  "outputs.exact_and_files": "no conformance evidence",
  "threads.list_reopen": "no conformance evidence",
  "errors.typed_states": "no conformance evidence",
};

function emptyResult(): ContractResult {
  return {
    appVersion: null,
    checkedAt: null,
    items: CONTRACT_ITEM_IDS.map((item) => ({
      item,
      passed: false,
      reason: NO_EVIDENCE[item],
      evidence: [],
    })),
    passed: [],
    failed: [...CONTRACT_ITEM_IDS],
    ok: false,
  };
}

/**
 * Derive the §3 item verdicts from a conformance report (the existing
 * /aai/conformance/<adapter> suite, run per route). Checks are matched
 * exactly by (area, name); a rule whose checks are missing or failed yields
 * a failed item with the reason — the report never gets rounded up to pass.
 */
export function contractFromReport(
  report: ConformanceReport,
  opts: { appVersion: string; checkedAt?: string }
): ContractResult {
  const out = emptyResult();
  out.appVersion = opts.appVersion;
  out.checkedAt = opts.checkedAt ?? new Date().toISOString();
  const byArea = new Map(report.areas.map((a) => [a.area, a]));

  for (const rule of ITEM_RULES) {
    const idx = out.items.findIndex((i) => i.item === rule.item);
    if (rule.checks.length === 0) {
      out.items[idx] = { ...out.items[idx], passed: false, reason: NO_EVIDENCE[rule.item], evidence: [] };
      continue;
    }
    const missing: string[] = [];
    const failed: string[] = [];
    const evidence: string[] = [];
    for (const [areaName, checkName] of rule.checks) {
      const area = byArea.get(areaName);
      const check = area?.checks.find((c) => c.name === checkName);
      const ref = `${areaName}: ${checkName}`;
      if (!check) {
        missing.push(ref);
        continue;
      }
      evidence.push(`${ref} (${check.status})`);
      if (check.status !== "pass") failed.push(`${ref}${check.reason ? ` — ${check.reason}` : ""}`);
    }
    const passed = missing.length === 0 && failed.length === 0;
    out.items[idx] = {
      ...out.items[idx],
      passed,
      reason: passed
        ? ""
        : [...failed, ...missing.map((m) => `${m} — check not run`)].join("; "),
      evidence,
    };
  }

  out.passed = out.items.filter((i) => i.passed).map((i) => i.item);
  out.failed = out.items.filter((i) => !i.passed).map((i) => i.item);
  out.ok = out.failed.length === 0;
  return out;
}

/** A route with no recorded conformance at all: every item failed, honestly. */
export function contractUnchecked(): ContractResult {
  return emptyResult();
}
