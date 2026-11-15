import type { Target, TargetComponent, TargetsDoc } from "../contract/types";
import targetsJson from "../contract/targets.json";

/** Cloudflare Free allows 50 subrequests per invocation; keep well under it. */
export const MAX_PROBES_PER_TICK = 30;

export const TARGETS: TargetsDoc = targetsJson as TargetsDoc;

/** Targets the Cloudflare vantage probes, capped at MAX_PROBES_PER_TICK (critical first). */
export function cloudflareTargets(doc: TargetsDoc = TARGETS): Target[] {
  const mine = doc.targets.filter((t) => t.vantages.includes("cloudflare"));
  const ordered = [...mine.filter((t) => t.critical), ...mine.filter((t) => !t.critical)];
  return ordered.slice(0, MAX_PROBES_PER_TICK);
}

export function targetsFor(doc: TargetsDoc, componentId: string): Target[] {
  return doc.targets.filter((t) => t.component_id === componentId);
}

export function componentById(doc: TargetsDoc): Map<string, TargetComponent> {
  return new Map(doc.components.map((c) => [c.id, c]));
}
