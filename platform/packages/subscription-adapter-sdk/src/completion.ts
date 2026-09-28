// §A1 — multi-signal completion detector; D11 — stall watchdog input.
import type { Page } from "playwright";
import type { SdkSelectorResolver } from "./selectors";

export class CompletionTimeout extends Error {
  constructor(timeoutMs: number) {
    super(`completion not detected within ${timeoutMs} ms`);
    this.name = "CompletionTimeout";
  }
}

export interface CompletionKeys {
  stop?: string;
  send?: string;
  streaming?: string;
  response?: string;
}

export interface CompletionOptions {
  stabilityMs?: number; // text-stability window; default 2000 (spec range 1500–3000)
  timeoutMs?: number; // default 120000
  pollIntervalMs?: number; // default 100
  now?: () => number;
  sleep?: (ms: number) => Promise<void>;
  keys?: CompletionKeys;
  // Providers whose Send button is not a completion signal — ChatGPT renders
  // it enabled, disabled, or not at all (voice button) depending on chat mode
  // and reply type. The send signal then only requires a non-empty reply;
  // stop/streaming absence + text stability still gate completion.
  ignoreSend?: boolean;
}

export interface CompletionResult {
  completed: true;
  last_change_at: string;
}

export interface CompletionSignals {
  stop_absent: boolean;
  send_enabled: boolean;
  stable: boolean;
  streaming_absent: boolean;
}

export interface CompletionTracker {
  pollOnce(): Promise<{ complete: boolean; signals: CompletionSignals }>;
  awaitCompletion(): Promise<CompletionResult>;
  lastChangeAt(): number;
  stalled(stallTimeoutS: number): boolean;
}

const DEFAULT_KEYS: Required<CompletionKeys> = {
  stop: "stop_button",
  send: "send_button",
  streaming: "streaming",
  response: "response",
};

const defaultSleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

export function createCompletionTracker(
  page: Page,
  resolver: SdkSelectorResolver,
  opts: CompletionOptions = {}
): CompletionTracker {
  const now = opts.now ?? (() => Date.now());
  const sleep = opts.sleep ?? defaultSleep;
  const stabilityMs = opts.stabilityMs ?? 2000;
  const timeoutMs = opts.timeoutMs ?? 120000;
  const pollIntervalMs = opts.pollIntervalMs ?? 100;
  const keys = { ...DEFAULT_KEYS, ...opts.keys };

  let lastChange = now();
  let lastText: string | null = null;

  async function pollOnce(): Promise<{ complete: boolean; signals: CompletionSignals }> {
    const responseLoc = await resolver.tryResolveLocator(keys.response);
    // Newest turn: on a continued thread .first() would watch a finished
    // earlier reply and never see the new one change.
    const text = responseLoc ? await responseLoc.last().innerText() : "";
    if (text !== lastText) {
      lastText = text;
      lastChange = now();
    }

    const stopLoc = await resolver.tryResolveLocator(keys.stop);
    const sendLoc = await resolver.tryResolveLocator(keys.send);
    const streamingLoc = await resolver.tryResolveLocator(keys.streaming);

    const signals: CompletionSignals = {
      stop_absent: stopLoc === null,
      send_enabled:
        opts.ignoreSend === true
          ? text.trim().length > 0
          : sendLoc !== null && (await sendLoc.first().isEnabled()),
      stable: now() - lastChange >= stabilityMs,
      streaming_absent: streamingLoc === null,
    };
    const complete =
      signals.stop_absent && signals.send_enabled && signals.stable && signals.streaming_absent;
    return { complete, signals };
  }

  return {
    pollOnce,
    lastChangeAt(): number {
      return lastChange;
    },
    stalled(stallTimeoutS: number): boolean {
      return now() - lastChange > stallTimeoutS * 1000;
    },
    async awaitCompletion(): Promise<CompletionResult> {
      const start = now();
      for (;;) {
        const { complete } = await pollOnce();
        if (complete) {
          return { completed: true, last_change_at: new Date(lastChange).toISOString() };
        }
        if (now() - start >= timeoutMs) throw new CompletionTimeout(timeoutMs);
        await sleep(pollIntervalMs);
      }
    },
  };
}

export async function awaitCompletion(
  page: Page,
  resolver: SdkSelectorResolver,
  opts: CompletionOptions = {}
): Promise<CompletionResult> {
  return createCompletionTracker(page, resolver, opts).awaitCompletion();
}
