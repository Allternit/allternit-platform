// §A3.3 — DeclarativeChatAdapter: chat-only providers as pure config
// (manifest + selectors YAML + thread-URL regex + banner pack), no provider TS.
import { createHash } from "node:crypto";
import type { ElementHandle, Locator, Page } from "playwright";
import type {
  AdapterEvent,
  AdapterManifest,
  AdapterRuntime,
  ExecutionContext,
  ProbeResult,
  ReconcileResult,
  SubmissionState,
  SubscriptionAdapter,
  Task,
  TaskAttempt,
  TaskError,
  ThreadSnapshot,
} from "@allternit/subscription-fabric-contracts";
import { detectAuthState, threadIdFromUrl } from "./auth";
import { createBannerClassifier, type BannerPattern } from "./banners";
import { createCompletionTracker, type CompletionOptions } from "./completion";
import { ComposerNotFilledError, composerState, fillComposer, submit } from "./composer";
import { extractLastAssistantTurn } from "./extract";
import { probe as probePage, type ProbeInput } from "./probe";
import {
  createHeartbeat,
  extractCounterBadge,
  extractPartialArtifacts,
  extractStepList,
  watchStreamingGrowth,
} from "./progress";
import { SelectorPack, createResolver, type SdkSelectorResolver } from "./selectors";
import type { SdkPageLease } from "./runtime";

// The worker's AdapterRuntime carries the real page inside the SDK boundary.
export interface SdkAdapterRuntime extends AdapterRuntime {
  page: Page;
}

export interface DeclarativeChatConfig {
  manifest: AdapterManifest;
  selectorsYaml: string;
  threadUrlPattern: RegExp;
  banners: BannerPattern[];
  criticalKeys?: string[]; // probe-critical subset (default: pack critical keys)
  sampleThreadUrl?: string; // conformance thread-ID check
  sampleThreadId?: string;
  completion?: CompletionOptions; // injectable clock/sleep for tests
  heartbeatIntervalMs?: number; // D11 default 15000
  // Before judging sign-in on a fresh page: how long to wait for the app to
  // draw a decisive marker (signed in, a check, or logged out). Single-page
  // apps (Kimi) render the signed-in UI after domcontentloaded. Default 8000.
  authSettleMs?: number;
  stallTimeoutS?: number; // §A8 default 90 for chat
  submitFallbackEnter?: boolean; // pack hint for submit()
  // How long to wait after Send for the provider to show it took the prompt
  // (default 10 s). Without that evidence the attempt stays sent_unconfirmed.
  ackTimeoutMs?: number;
}

// §A8 — stalled/timeout are retryable only when the submit never happened.
export function stalledError(state: SubmissionState, detail: string): TaskError {
  return {
    class: "stalled",
    scope: "task",
    retryable: state === "not_sent",
    fallback_eligible: true,
    cooldown_s: null,
    user_action: null,
    detail,
    evidence_ref: null,
  };
}

export function timeoutError(state: SubmissionState, detail: string): TaskError {
  return {
    class: "timeout",
    scope: "task",
    retryable: state === "not_sent",
    fallback_eligible: true,
    cooldown_s: null,
    user_action: null,
    detail,
    evidence_ref: null,
  };
}

function sdkPage(lease: ExecutionContext["page"]): Page {
  return (lease as SdkPageLease).page;
}

export class DeclarativeChatAdapter implements SubscriptionAdapter {
  readonly manifest: AdapterManifest;
  readonly pack: SelectorPack;
  private runtimePage: Page | null = null;

  constructor(private readonly config: DeclarativeChatConfig) {
    this.manifest = config.manifest;
    this.pack = SelectorPack.fromYaml(config.selectorsYaml);
  }

  probeInput(): ProbeInput {
    return { auth: this.manifest.auth, criticalKeys: this.config.criticalKeys };
  }

  async attach(ctx: AdapterRuntime): Promise<void> {
    this.runtimePage = (ctx as SdkAdapterRuntime).page ?? null;
  }

  async detach(): Promise<void> {
    this.runtimePage = null;
  }

  /** The worker-provided page while attached (for non-spending reads). */
  protected attachedPage(): Page | null {
    return this.runtimePage;
  }

  // §A3.4 — non-spending canary over the worker-provided page.
  async probe(_signal: AbortSignal): Promise<ProbeResult> {
    if (!this.runtimePage) throw new Error("probe called before attach()");
    return probePage(
      this.runtimePage,
      createResolver(this.runtimePage, this.pack),
      this.pack,
      this.probeInput()
    );
  }

  // §A1 — streaming execute; never retries a submit.
  async *execute(task: Task, ctx: ExecutionContext): AsyncIterable<AdapterEvent> {
    const page = sdkPage(ctx.page);
    const resolver = ctx.selectors as SdkSelectorResolver;
    const cfg = this.config;
    const now = cfg.completion?.now ?? (() => Date.now());
    const sleep =
      cfg.completion?.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
    const pollIntervalMs = cfg.completion?.pollIntervalMs ?? 100;
    const timeoutMs = cfg.completion?.timeoutMs ?? 120000;
    const stallTimeoutS = cfg.stallTimeoutS ?? 90;
    const heartbeatIntervalMs = cfg.heartbeatIntervalMs ?? 15000;
    const poolId =
      this.manifest.capabilities.find((c) => c.id === task.capability)?.pool_id ??
      this.manifest.capabilities[0]?.pool_id ??
      "unknown";

    // Let a single-page app draw before judging: stop as soon as the page is
    // signed in, shows a check, or shows its logged-out marker.
    const settleUntil = now() + (cfg.authSettleMs ?? 8000);
    for (;;) {
      const decisive = [this.manifest.auth.logged_in_probe, "challenge", "logged_out_probe"];
      let found = false;
      for (const key of decisive) {
        if (await resolver.tryResolveLocator(key).catch(() => null)) {
          found = true;
          break;
        }
      }
      if (found || now() >= settleUntil) break;
      await sleep(250);
    }

    // §A5/Critical #5 — a challenge interstitial halts immediately, never retried.
    if (await resolver.tryResolveLocator("challenge")) {
      yield {
        t: "needs_user",
        reason: "challenge",
        message: "provider presented a verification interstitial",
      };
      return;
    }
    if (
      (await detectAuthState(page, resolver, {
        probeKey: this.manifest.auth.logged_in_probe,
      })) !== "ready"
    ) {
      yield { t: "needs_user", reason: "auth", message: "not logged in" };
      return;
    }

    const classifier = createBannerClassifier(cfg.banners);
    const seenQuotaKinds = new Set<string>();
    const scanBanners = async (): Promise<AdapterEvent[]> => {
      const out: AdapterEvent[] = [];
      const bannerLoc = await resolver.tryResolveLocator("banner");
      if (!bannerLoc) return out;
      for (const el of await bannerLoc.all()) {
        const hit = classifier.classify(await el.innerText());
        if (hit && !seenQuotaKinds.has(hit.kind)) {
          seenQuotaKinds.add(hit.kind);
          out.push({
            t: "quota.signal",
            pool_id: poolId,
            signal: {
              kind: hit.kind,
              raw_excerpt: hit.raw_excerpt,
              observed_at: new Date(now()).toISOString(),
              task_id: task.task_id,
            },
          });
        }
      }
      return out;
    };
    yield* await scanBanners();

    // Replies already on the page (chat.continue): live text streams only
    // once a new one appears, never the previous turn.
    const repliesBefore = await countReplies(resolver);

    await ctx.pacing.beforeTask();
    let composer;
    try {
      composer = await fillComposer(page, resolver, task.prompt);
    } catch (err) {
      if (!(err instanceof ComposerNotFilledError)) throw err;
      // Nothing was sent: safe to retry once the pack matches the page again.
      yield { t: "error", error: composerDriftError(err.detail) };
      return;
    }
    const userTurnsBefore = await countKey(resolver, "user_turn");
    await ctx.pacing.beforeAction();
    const composerEl = await readyToSend(composer, task.prompt);
    if (typeof composerEl === "string") {
      yield { t: "error", error: composerDriftError(composerEl) };
      return;
    }
    await ctx.markSubmitted(null); // §A1: sent_unconfirmed BEFORE Send is clicked
    const via = await submit(page, resolver, { fallback: cfg.submitFallbackEnter ? "enter" : undefined });
    // §A1: acknowledged only on the provider's own evidence.
    const ack = await confirmSend(page, resolver, ctx, {
      composer: composerEl,
      prompt: task.prompt,
      threadUrlPattern: cfg.threadUrlPattern,
      userTurnsBefore,
      repliesBefore,
      timeoutMs: cfg.ackTimeoutMs,
      pollIntervalMs,
      now,
      sleep,
    });
    let threadId = ack.threadId;
    // Stall/timeout details say how the send went, so a live failure carries
    // its own evidence.
    const sendNote = `; sent by ${via}, ${ack.evidence ? `acknowledged by ${ack.evidence}` : "not acknowledged"}`;
    const url = page.url();
    yield {
      t: "submitted",
      provider_thread_id: threadId,
      provider_url: url.startsWith("about:") ? null : url,
    };

    // D11 — progress/heartbeat stream while awaiting completion.
    const tracker = createCompletionTracker(page, resolver, { ...cfg.completion, now, sleep });
    const pending: AdapterEvent[] = [];
    const hb = createHeartbeat(
      (e) => pending.push(e),
      heartbeatIntervalMs,
      { now, lastChangeAt: () => tracker.lastChangeAt() }
    );
    const watcher = watchStreamingGrowth(page, resolver, (e) => pending.push(e as AdapterEvent));
    const startedAt = now();
    let lastHb = now();

    await extractStepList(page, resolver, (e) => pending.push(e as AdapterEvent));
    await extractCounterBadge(page, resolver, (e) => pending.push(e as AdapterEvent));
    await extractPartialArtifacts(page, resolver, {
      provider: this.manifest.provider,
      emit: (e) => pending.push(e as AdapterEvent),
    });

    // Live reply text: sample the new reply's markdown and emit only what two
    // consecutive samples agree on — a partial code block renders with its
    // closing fence, which moves as the block grows, so it is held back.
    const replyId = `reply-${task.task_id}`;
    const runId = `run-${ctx.attempt.attempt_no}`;
    let replyStarted = false;
    let emitted = "";
    let previousSample: string | null = null;
    let lastSampleAt = 0;
    const emitText = (text: string): AdapterEvent[] => {
      if (!text) return [];
      const out: AdapterEvent[] = [];
      if (!replyStarted) {
        replyStarted = true;
        out.push({ t: "reply", event: { type: "reply.started", replyId, runId, ts: now() } });
      }
      emitted += text;
      out.push({
        t: "reply",
        event: { type: "reply.text.delta", replyId, runId, itemId: "item-1", delta: text, ts: now() },
      });
      return out;
    };
    const sampleReply = async (): Promise<AdapterEvent[]> => {
      if (now() - lastSampleAt < LIVE_SAMPLE_MS) return [];
      lastSampleAt = now();
      if ((await countReplies(resolver)) <= repliesBefore) return [];
      let sample: string;
      try {
        sample = await extractLastAssistantTurn(page, resolver);
      } catch {
        return []; // the node re-rendered mid-read; next sample
      }
      const stable = previousSample === null ? "" : commonPrefix(previousSample, sample);
      previousSample = sample;
      if (stable.length <= emitted.length || !stable.startsWith(emitted)) return [];
      return emitText(stable.slice(emitted.length));
    };

    for (;;) {
      while (pending.length > 0) yield pending.shift() as AdapterEvent;
      for (const e of await sampleReply()) yield e;
      // Providers route to the thread URL a beat after Send (ChatGPT: / →
      // /c/<id>). Persist the id once it appears so crash reconcile can
      // reopen the mapped thread (the SDK keeps state at acknowledged).
      if (threadId === null) {
        threadId = threadIdFromUrl(page.url(), cfg.threadUrlPattern);
        if (threadId !== null) await ctx.markSubmitted(threadId);
      }
      const { complete } = await tracker.pollOnce();
      if (complete) break;
      if (tracker.stalled(stallTimeoutS)) {
        yield {
          t: "error",
          error: stalledError(
            ctx.attempt.submission_state,
            `no DOM change for ${stallTimeoutS}s${sendNote}${await pageShape(page)}`
          ),
        };
        return;
      }
      if (now() - startedAt >= timeoutMs) {
        yield {
          t: "error",
          error: timeoutError(
            ctx.attempt.submission_state,
            `no completion within ${timeoutMs}ms${sendNote}${await pageShape(page)}`
          ),
        };
        return;
      }
      await watcher.sample();
      if (now() - lastHb >= heartbeatIntervalMs) {
        lastHb = now();
        hb.tick();
      }
      await sleep(pollIntervalMs);
    }
    while (pending.length > 0) yield pending.shift() as AdapterEvent;

    const hasResponse = (await resolver.tryResolveLocator("response")) !== null;
    const text = hasResponse ? await extractLastAssistantTurn(page, resolver) : undefined;
    // The rest of the reply. `done.text` stays the authoritative full text;
    // a final render that no longer extends what streamed (rare) streams
    // nothing more.
    if (text && text.startsWith(emitted)) {
      for (const e of emitText(text.slice(emitted.length))) yield e;
    }
    yield* await scanBanners();
    yield { t: "done", outcome: "success", text };
  }

  // §A1/Critical #2 — reconcile a sent_unconfirmed attempt; never resubmits.
  async reconcile(attempt: TaskAttempt, ctx: ExecutionContext): Promise<ReconcileResult> {
    const page = sdkPage(ctx.page);
    const resolver = ctx.selectors as SdkSelectorResolver;
    const urlId = threadIdFromUrl(page.url(), this.config.threadUrlPattern);
    const hasResponse = (await resolver.tryResolveLocator("response")) !== null;
    if (hasResponse && attempt.provider_thread_id && urlId === attempt.provider_thread_id) {
      return { outcome: "acknowledged", provider_thread_id: urlId };
    }
    if (!hasResponse) return { outcome: "not_found" };
    return { outcome: "ambiguous", detail: "response present but thread id not confirmed" };
  }

  async readThread(providerThreadId: string, ctx: ExecutionContext): Promise<ThreadSnapshot> {
    const page = sdkPage(ctx.page);
    const resolver = ctx.selectors as SdkSelectorResolver;
    const markdown = await extractLastAssistantTurn(page, resolver);
    const turns = await (await resolver.resolveLocator("response")).count();
    return {
      provider_thread_id: providerThreadId,
      turn_count: turns,
      last_turn_fingerprint: createHash("sha256").update(markdown).digest("hex"),
      observed_at: new Date().toISOString(),
    };
  }
}

// Live reply sampling cadence: markdown extraction is a page round trip, so
// it runs at most this often however fast the completion poll is.
const LIVE_SAMPLE_MS = 300;

/**
 * Evidence for a stall or timeout: the conversation area's structure (tag,
 * role and data-* attributes of message-like elements), never any text the
 * person or the provider wrote. When a provider changes its markup this is
 * what shows which selectors stopped matching, straight from the task error.
 */
const PAGE_SHAPE_SCRIPT = `(() => {
  var root = document.querySelector("main") || document.body;
  var known = root.querySelectorAll(
    "[data-message-author-role],[data-conversation-role],[data-turn],[data-testid],[data-message-id],article,[role=article],[role=presentation]"
  );
  var all = known.length ? Array.prototype.slice.call(known) : Array.prototype.slice.call(root.querySelectorAll("*")).filter(function (el) {
    return el.hasAttribute("role") || Array.prototype.some.call(el.attributes, function (a) { return a.name.indexOf("data-") === 0; });
  });
  var seen = {};
  var out = [];
  for (var i = 0; i < all.length && out.length < 40; i++) {
    var el = all[i];
    var parts = [el.tagName.toLowerCase()];
    for (var j = 0; j < el.attributes.length; j++) {
      var a = el.attributes[j];
      if (a.name === "role" || a.name.indexOf("data-") === 0) parts.push(a.name + "=" + a.value.slice(0, 40));
    }
    var d = parts.join(" ");
    if (seen[d]) continue;
    seen[d] = true;
    out.push(d);
  }
  return out.join(" | ");
})()`;

export async function pageShape(page: Page, max = 1200): Promise<string> {
  try {
    // A string, not a function: the gateway runs under tsx/esbuild, which
    // wraps named inner functions in a __name() helper that doesn't exist in
    // the provider's page (live: "ReferenceError: __name is not defined").
    const shape = (await page.evaluate(PAGE_SHAPE_SCRIPT)) as string;
    return `; page shape: ${shape ? shape.slice(0, max) : "(no data-* or role elements)"}`;
  } catch (err) {
    return `; page shape unavailable: ${(err instanceof Error ? err.message : String(err)).slice(0, 120)}`;
  }
}

export interface ConfirmSendOptions {
  composer: ElementHandle; // the element that held the prompt at Send
  prompt: string;
  threadUrlPattern: RegExp;
  userTurnsBefore: number;
  repliesBefore: number;
  timeoutMs?: number; // default 10000
  pollIntervalMs: number;
  now: () => number;
  sleep: (ms: number) => Promise<void>;
}

export type SendEvidence = "thread_url" | "user_turn" | "reply" | "composer_cleared";

/**
 * Right before Send: the composer that took the prompt is still the element
 * on the page and still holds it. Returns its handle, or why not (nothing has
 * been sent yet, so the caller fails as drift).
 */
export async function readyToSend(composer: Locator, prompt: string): Promise<ElementHandle | string> {
  const el = await composer.elementHandle({ timeout: 2000 }).catch(() => null);
  if (!el) return "the composer disappeared before Send";
  const state = await composerState(el, prompt);
  if (state === "holds") return el;
  return state === "replaced" ? "the composer was replaced before Send" : "the composer lost the prompt before Send";
}

/**
 * §A1 — after Send, marks the attempt acknowledged only on the provider's own
 * evidence: a thread URL, a new user turn, a new reply, or the SAME composer
 * element emptied (a replaced editor proves nothing — live: ChatGPT swapped
 * its editor and the empty new one looked like a send). Without evidence the
 * attempt stays sent_unconfirmed.
 */
export async function confirmSend(
  page: Page,
  resolver: SdkSelectorResolver,
  ctx: Pick<ExecutionContext, "markSubmitted" | "log">,
  opts: ConfirmSendOptions
): Promise<{ threadId: string | null; evidence: SendEvidence | null }> {
  const timeoutMs = opts.timeoutMs ?? 10000;
  const deadline = opts.now() + timeoutMs;
  // Bounded by rounds too: injected clocks in tests may not advance.
  const rounds = Math.ceil(timeoutMs / Math.max(opts.pollIntervalMs, 1)) + 1;
  for (let round = 0; ; round++) {
    const threadId = threadIdFromUrl(page.url(), opts.threadUrlPattern);
    const evidence: SendEvidence | null =
      threadId !== null
        ? "thread_url"
        : (await countKey(resolver, "user_turn")) > opts.userTurnsBefore
          ? "user_turn"
          : (await countReplies(resolver)) > opts.repliesBefore
            ? "reply"
            : (await composerState(opts.composer, opts.prompt)) === "cleared"
              ? "composer_cleared"
              : null;
    if (evidence) {
      await ctx.markSubmitted(threadId);
      ctx.log.info(`send acknowledged by ${evidence}`);
      return { threadId, evidence };
    }
    if (opts.now() >= deadline || round >= rounds) {
      ctx.log.warn("send not confirmed by the provider; attempt stays sent_unconfirmed");
      return { threadId, evidence: null };
    }
    await opts.sleep(opts.pollIntervalMs);
  }
}

async function countKey(resolver: SdkSelectorResolver, key: string): Promise<number> {
  const loc = await resolver.tryResolveLocator(key);
  return loc ? loc.count() : 0;
}

export function composerDriftError(detail: string): TaskError {
  return {
    class: "provider_ui_changed",
    scope: "adapter",
    retryable: true,
    fallback_eligible: true,
    cooldown_s: null,
    user_action: null,
    detail: `the prompt didn't land in the composer: ${detail}`,
    evidence_ref: null,
  };
}

async function countReplies(resolver: SdkSelectorResolver): Promise<number> {
  const locator = await resolver.tryResolveLocator("response");
  return locator ? await locator.count() : 0;
}

function commonPrefix(a: string, b: string): string {
  const n = Math.min(a.length, b.length);
  let i = 0;
  while (i < n && a.charCodeAt(i) === b.charCodeAt(i)) i++;
  return a.slice(0, i);
}
