// D11 — generic progress extractors driven by the `progress:` selector group.
import type { Page } from "playwright";
import type { AdapterEvent, ProviderId } from "@allternit/subscription-fabric-contracts";
import type { SdkSelectorResolver } from "./selectors";

export type ProgressHeartbeat = Extract<AdapterEvent, { t: "progress.heartbeat" }>;
export type ProgressEvent = Extract<AdapterEvent, { t: "progress" }>;
export type PartialArtifactEvent = Extract<AdapterEvent, { t: "artifact.partial" }>;
export type ProgressEmit = (event: AdapterEvent) => void;

export const PROGRESS_KEYS = {
  steps: "progress:steps",
  counter: "progress:counter",
  partialArtifacts: "progress:partial_artifacts",
  response: "response",
} as const;

// Step-list items: one `progress` event per step; fraction = done/total when
// the items carry a derivable state (data-state="done|active|pending").
export async function extractStepList(
  page: Page,
  resolver: SdkSelectorResolver,
  emit?: ProgressEmit
): Promise<ProgressEvent[]> {
  const locator = await resolver.tryResolveLocator(PROGRESS_KEYS.steps);
  if (!locator) return [];
  const items = await locator.all();
  const states = await Promise.all(items.map((i) => i.getAttribute("data-state")));
  const done = states.filter((s) => s === "done").length;
  const fraction =
    items.length > 0 && states.every((s) => s !== null) ? done / items.length : undefined;
  const events: ProgressEvent[] = [];
  for (const item of items) {
    const label = (await item.innerText()).trim();
    if (!label) continue;
    const event: ProgressEvent =
      fraction === undefined ? { t: "progress", label } : { t: "progress", label, fraction };
    events.push(event);
    emit?.(event);
  }
  return events;
}

// Counter badges: "N/M …" → fraction; "N sources" style → label only.
export async function extractCounterBadge(
  page: Page,
  resolver: SdkSelectorResolver,
  emit?: ProgressEmit
): Promise<ProgressEvent | null> {
  const locator = await resolver.tryResolveLocator(PROGRESS_KEYS.counter);
  if (!locator) return null;
  const text = (await locator.first().innerText()).trim();
  if (!text) return null;
  const ratio = /(\d+)\s*\/\s*(\d+)/.exec(text);
  const event: ProgressEvent =
    ratio && Number(ratio[2]) > 0
      ? { t: "progress", label: text, fraction: Number(ratio[1]) / Number(ratio[2]) }
      : { t: "progress", label: text };
  emit?.(event);
  return event;
}

export interface StreamingGrowthWatcher {
  // Re-reads the response text; emits a progress event when it grew.
  sample(): Promise<ProgressEvent | null>;
  lastLength(): number;
}

// Streaming-text growth: length deltas of the response node → progress events.
export function watchStreamingGrowth(
  page: Page,
  resolver: SdkSelectorResolver,
  emit: ProgressEmit,
  opts: { key?: string } = {}
): StreamingGrowthWatcher {
  const key = opts.key ?? PROGRESS_KEYS.response;
  let lastLen: number | null = null;
  return {
    lastLength(): number {
      return lastLen ?? 0;
    },
    async sample(): Promise<ProgressEvent | null> {
      const locator = await resolver.tryResolveLocator(key);
      // The newest reply: in a thread, .first() is an earlier turn that never grows.
      const text = locator ? await locator.last().innerText() : "";
      const len = text.length;
      if (lastLen === null) {
        lastLen = len;
        return null;
      }
      if (len <= lastLen) {
        lastLen = len;
        return null;
      }
      const delta = len - lastLen;
      lastLen = len;
      const event: ProgressEvent = { t: "progress", label: `streaming (+${delta} chars)` };
      emit(event);
      return event;
    },
  };
}

// Partial artifacts (e.g. slide thumbnails appearing mid-run) → artifact.partial.
export async function extractPartialArtifacts(
  page: Page,
  resolver: SdkSelectorResolver,
  opts: { provider: ProviderId; key?: string; emit?: ProgressEmit }
): Promise<PartialArtifactEvent[]> {
  const locator = await resolver.tryResolveLocator(opts.key ?? PROGRESS_KEYS.partialArtifacts);
  if (!locator) return [];
  const items = await locator.all();
  const events: PartialArtifactEvent[] = [];
  for (const item of items) {
    const url = await item.getAttribute("src");
    if (!url) continue;
    const id = (await item.getAttribute("data-artifact-id")) ?? url;
    const event: PartialArtifactEvent = {
      t: "artifact.partial",
      ref: {
        provider: opts.provider,
        provider_artifact_id: id,
        provider_url: url,
        provider_url_expires_at: null,
      },
    };
    events.push(event);
    opts.emit?.(event);
  }
  return events;
}

export interface HeartbeatHandle {
  tick(): ProgressHeartbeat;
  start(): void;
  stop(): void;
}

// D11 — 15 s heartbeat with growing elapsed_s; the watchdog consumes last_change_at.
export function createHeartbeat(
  emit: (event: ProgressHeartbeat) => void,
  intervalMs = 15000,
  deps: { now?: () => number; lastChangeAt?: () => number } = {}
): HeartbeatHandle {
  const now = deps.now ?? (() => Date.now());
  const startedAt = now();
  let timer: ReturnType<typeof setInterval> | null = null;
  function tick(): ProgressHeartbeat {
    const lastChange = deps.lastChangeAt ? deps.lastChangeAt() : startedAt;
    const event: ProgressHeartbeat = {
      t: "progress.heartbeat",
      elapsed_s: (now() - startedAt) / 1000,
      last_change_at: new Date(lastChange).toISOString(),
    };
    emit(event);
    return event;
  }
  return {
    tick,
    start(): void {
      if (timer === null) timer = setInterval(tick, intervalMs);
    },
    stop(): void {
      if (timer !== null) clearInterval(timer);
      timer = null;
    },
  };
}
