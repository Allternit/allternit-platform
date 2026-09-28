// chatgpt-web adapter — §A3.3: DeclarativeChatAdapter base + code hooks for
// image.generate, D5 temp-chat default, chat.continue divergence check
// (Critical #7), fingerprint reconcile (Critical #2), SingletonLock →
// profile_locked (fix #6). Selectors are v1-unverified (see selectors/v1.yaml).
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { load as yamlLoad } from "js-yaml";
import type { Page } from "playwright";
import {
  adapterManifestSchema,
  type AdapterEvent,
  type AdapterManifest,
  type ExecutionContext,
  type ReconcileResult,
  type Task,
  type TaskAttempt,
  type TaskError,
} from "@allternit/subscription-fabric-contracts";
import {
  DeclarativeChatAdapter,
  captureImages,
  createCompletionTracker,
  createHeartbeat,
  createResolver,
  detectAuthState,
  fillComposer,
  stalledError,
  submit,
  threadIdFromUrl,
  timeoutError,
  type DeclarativeChatConfig,
  type SdkPageLease,
  type SdkSelectorResolver,
} from "@allternit/subscription-adapter-sdk";

// ChatGPT first routes a new chat to a provisional /c/local-… id before the
// server id arrives; that one is not reopenable, so it never matches.
// Project chats live under /g/<project>/c/<id>; the id is the same thread id.
export const THREAD_URL_PATTERN = /^https:\/\/chatgpt\.com\/(?:g\/[\w-]+\/)?c\/(?!local-)([\w-]+)/;
const PROJECT_PAGE_PATTERN = /^https:\/\/chatgpt\.com\/g\/[\w-]+\/project/;
// Marks images already in a reused chat so only this run's images are
// watched and captured.
const SEEN_ATTR = "data-allternit-seen";

export function loadManifest(): AdapterManifest {
  const raw = yamlLoad(
    readFileSync(new URL("./manifest.yaml", import.meta.url), "utf8")
  );
  return adapterManifestSchema.parse(raw);
}

export function selectorsYaml(): string {
  return readFileSync(new URL("./selectors/v1.yaml", import.meta.url), "utf8");
}

export function chatGPTWebConfig(
  overrides: Partial<DeclarativeChatConfig> = {}
): DeclarativeChatConfig {
  return {
    manifest: loadManifest(),
    selectorsYaml: selectorsYaml(),
    threadUrlPattern: THREAD_URL_PATTERN,
    banners: [
      { kind: "limit_banner", pattern: /you'?ve reached (your )?(usage )?limit/i },
      { kind: "limit_banner", pattern: /approaching (your )?(usage )?limit/i },
      { kind: "slow_mode", pattern: /slower (responses|mode)|slow mode/i },
      { kind: "reset_notice", pattern: /(quota|limit|usage) resets? (at|in)/i },
    ],
    // send_button is not probed: the live UI renders it only once the
    // composer has text (idle shows the voice button), so an idle probe would
    // always report ui_drift. Submit resolves it after fillComposer.
    criticalKeys: ["composer", "logged_in_probe"],
    sampleThreadUrl: "https://chatgpt.com/c/68f7c000-aaaa-bbbb-cccc-dddddddddddd",
    sampleThreadId: "68f7c000-aaaa-bbbb-cccc-dddddddddddd",
    submitFallbackEnter: true,
    ...overrides,
    // Merge, don't replace: a clock/timing override must not drop the
    // live-UI completion flag.
    completion: { ignoreSend: true, ...overrides.completion },
  };
}

// §A2/Critical #2 — fingerprint of a provider-side user turn; mirrors the
// gateway worker's promptFingerprint for a prompt-only task (no inputs).
export function userTurnFingerprint(text: string): string {
  const normalized = text.replace(/\s+/g, " ").trim();
  return createHash("sha256").update(normalized + "\n").digest("hex");
}

export type DivergencePolicy = "adopt" | "fork" | "fail";
export type DivergenceAction = "proceed" | "fork" | "fail";

// Critical #7 — on a last-turn-fingerprint mismatch, the mapping's
// on_divergence policy decides: adopt continues, fork/fail stop the submit.
export function resolveDivergence(
  expected: string | null,
  observed: string,
  policy: DivergencePolicy
): DivergenceAction {
  if (expected === null || expected === observed) return "proceed";
  return policy === "adopt" ? "proceed" : policy;
}

// fix #6 — a profile held by another process is profile_locked, not a crash.
export function isProfileLockError(err: unknown): boolean {
  const msg = err instanceof Error ? err.message : String(err);
  return /SingletonLock|user data directory is already in use|profile (is )?locked/i.test(msg);
}

export function profileLockedError(detail: string): TaskError {
  return {
    class: "profile_locked",
    scope: "account",
    retryable: true, // after the lock is released
    fallback_eligible: true,
    cooldown_s: null,
    user_action: "Close the provider window holding the profile, then retry",
    detail,
    evidence_ref: null,
  };
}

function sdkPage(lease: ExecutionContext["page"]): Page {
  return (lease as SdkPageLease).page;
}

export interface ChatGPTWebOptions {
  // D5 — temp-chat ON by default for stateless chat.create tasks.
  tempChat?: boolean;
  // chat.create and image.generate start from the origin root — a fresh,
  // regular new chat — instead of whatever the lane page is showing (a
  // previous task's thread, or a temp chat). Default true; fixture tests that
  // load a page directly pass false.
  freshChat?: boolean;
}

export type ChatGPTWebConfigOverrides = Partial<DeclarativeChatConfig>;

export class ChatGPTWebAdapter extends DeclarativeChatAdapter {
  private readonly cfg: DeclarativeChatConfig;

  constructor(
    private readonly opts: ChatGPTWebOptions = {},
    configOverrides: ChatGPTWebConfigOverrides = {}
  ) {
    const config = chatGPTWebConfig(configOverrides);
    super(config);
    this.cfg = config;
  }

  override async *execute(task: Task, ctx: ExecutionContext): AsyncIterable<AdapterEvent> {
    try {
      if (task.capability === "image.generate") {
        yield* this.executeImage(task, ctx);
        return;
      }
      if (task.capability === "chat.continue") {
        const gate = await this.prepareContinue(task, ctx);
        if (gate) {
          yield gate;
          return;
        }
      } else {
        // Never type into the page as left by the previous task: a chat.create
        // there would land in that task's thread (live: a stateless prompt
        // appended to a mapped fabric thread) or in its temp chat.
        await this.openFreshChat(ctx);
        if (this.opts.tempChat !== false && !task.thread_id) {
          // D5: temporary chat is for STATELESS tasks only. A task on a fabric
          // thread must land in a reopenable chat so chat.continue can follow.
          await this.enableTempChat(ctx);
        }
      }
      yield* super.execute(task, ctx);
    } catch (err) {
      if (isProfileLockError(err)) {
        yield {
          t: "error",
          error: profileLockedError(err instanceof Error ? err.message : String(err)),
        };
        return;
      }
      throw err;
    }
  }

  // Live UI: "+" → "Create image" menu entry → composer chip "Remove Create
  // image". Legacy direct toggle kept as a fallback (fixtures, older UI).
  private async enableImageMode(ctx: ExecutionContext): Promise<boolean> {
    const resolver = ctx.selectors as SdkSelectorResolver;
    if (await resolver.tryResolveLocator("image_mode_active")) return true;
    const plus = await resolver.tryResolveLocator("composer_plus");
    if (plus) {
      await ctx.pacing.beforeAction();
      await plus.first().click();
      const item = await resolver.tryResolveLocator("image_menu_item");
      if (item) {
        await ctx.pacing.beforeAction();
        await item.last().click();
        const page = sdkPage(ctx.page);
        for (let i = 0; i < 20; i++) {
          if (await resolver.tryResolveLocator("image_mode_active")) return true;
          await page.waitForTimeout(100);
        }
      }
      await sdkPage(ctx.page).keyboard.press("Escape");
    }
    const toggle = await resolver.tryResolveLocator("capability:image_tool_toggle");
    if (!toggle) return false;
    if ((await toggle.first().getAttribute("aria-pressed")) !== "true") {
      await ctx.pacing.beforeAction();
      await toggle.first().click();
    }
    return true;
  }

  private async openFreshChat(ctx: ExecutionContext): Promise<void> {
    if (this.opts.freshChat === false) return;
    await sdkPage(ctx.page).goto(this.manifest.origins[0] ?? "https://chatgpt.com/", {
      waitUntil: "domcontentloaded",
    });
  }

  // Image-chat policy. options.image_chat_url: the account's active image
  // chat (reused unless it is gone). options.image_project: the provider
  // project a new image chat opens in (created on first use). Neither → a
  // plain new chat. freshChat: false (fixture pages) skips navigation.
  // Returns true when it reopened an existing chat.
  private async openImageChat(task: Task, ctx: ExecutionContext): Promise<boolean> {
    if (this.opts.freshChat === false) return false;
    const page = sdkPage(ctx.page);
    const origin = this.manifest.origins[0] ?? "https://chatgpt.com";
    const chatUrl = typeof task.options.image_chat_url === "string" ? task.options.image_chat_url : null;
    const chatId = chatUrl && chatUrl.startsWith(`${origin}/`) ? threadIdFromUrl(chatUrl, THREAD_URL_PATTERN) : null;
    if (chatUrl && chatId) {
      await page.goto(chatUrl, { waitUntil: "domcontentloaded" });
      if (threadIdFromUrl(page.url(), THREAD_URL_PATTERN) === chatId) {
        await this.waitForThreadRender(ctx);
        await this.waitForGalleryStable(ctx);
        return true;
      }
      ctx.log.warn("image chat is gone; opening a new one", { image_chat_url: chatUrl });
    }
    const project = typeof task.options.image_project === "string" ? task.options.image_project.trim() : "";
    const projectUrl = typeof task.options.image_project_url === "string" ? task.options.image_project_url : null;
    if (project && projectUrl && projectUrl.startsWith(`${origin}/`)) {
      await page.goto(projectUrl, { waitUntil: "domcontentloaded" });
      if (PROJECT_PAGE_PATTERN.test(page.url())) return false;
      ctx.log.warn("image project page is gone; looking the project up", { image_project_url: projectUrl });
    }
    await this.openFreshChat(ctx);
    if (project && !(await this.openProject(ctx, project))) {
      ctx.log.warn("image project unavailable; using a plain new chat", { project });
      await this.openFreshChat(ctx);
    }
    return false;
  }

  // Sidebar Projects section → the named project, created when missing
  // (live UI 2026-09-28: section[data-app-action-sidebar-section-heading=
  // Projects], hover-revealed "Add new project", dialog "Create project").
  // Ends on the project page, whose composer starts a chat in the project.
  private async openProject(ctx: ExecutionContext, name: string): Promise<boolean> {
    const page = sdkPage(ctx.page);
    const resolver = ctx.selectors as SdkSelectorResolver;
    // The sidebar renders after the load event, and its project list after
    // that: wait for the section, then give the list time to fill before
    // concluding the project is missing (a premature create duplicates it).
    let section = null;
    for (let i = 0; i < 40 && !section; i++) {
      section = await resolver.tryResolveLocator("projects_section");
      if (!section) await page.waitForTimeout(250);
    }
    if (!section) {
      ctx.log.warn("projects sidebar section not found");
      return false;
    }
    const link = section.first().getByRole("link", { name, exact: true });
    for (let i = 0; i < 20 && (await link.count()) === 0; i++) await page.waitForTimeout(250);
    if ((await link.count()) > 0) {
      await ctx.pacing.beforeAction();
      await link.first().click();
    } else {
      const create = await resolver.tryResolveLocator("project_create");
      if (!create) {
        ctx.log.warn("project create button not found");
        return false;
      }
      // The button only moves into the sidebar while the section's TITLE row
      // is hovered (live: unhovered it sits at x=350, outside the 330px
      // sidebar, so a forced click misses). Hover the title, click normally;
      // a DOM click is the fallback.
      await ctx.pacing.beforeAction();
      await section.first().locator("[data-app-action-sidebar-section-toggle]").first().hover();
      try {
        await create.first().click({ timeout: 3000 });
      } catch {
        await create.first().evaluate((el) => (el as HTMLElement).click());
      }
      const dialog = page.getByRole("dialog", { name: /create project/i });
      const nameBox = dialog.getByRole("textbox", { name: /project name/i });
      try {
        await nameBox.waitFor({ timeout: 5000 });
      } catch {
        ctx.log.warn("create-project dialog did not open");
        return false;
      }
      await nameBox.fill(name);
      await ctx.pacing.beforeAction();
      await dialog.getByRole("button", { name: /^create project$/i }).click();
    }
    for (let i = 0; i < 60; i++) {
      if (PROJECT_PAGE_PATTERN.test(page.url())) return true;
      await page.waitForTimeout(250);
    }
    ctx.log.warn("project page did not open", { url: page.url() });
    return false;
  }

  // A reused image chat renders its earlier images lazily; mark them only
  // once they are in the DOM and the count holds — it always has at least one
  // (live: a 0 == 0 "stable" read let the earlier image be re-captured).
  private async waitForGalleryStable(ctx: ExecutionContext, capMs = 15000): Promise<void> {
    const page = sdkPage(ctx.page);
    const resolver = ctx.selectors as SdkSelectorResolver;
    let last = -1;
    for (let waited = 0; waited < capMs; waited += 500) {
      const gallery = await resolver.tryResolveLocator("image_result");
      const count = gallery ? await gallery.locator("img").count() : 0;
      if (count > 0 && count === last) return;
      last = count;
      await page.waitForTimeout(500);
    }
    ctx.log.warn("reused image chat showed no earlier images", { url: page.url() });
  }

  // D5 — click the temp-chat toggle unless already on; plans without the
  // toggle just run in normal history (documented in README).
  private async enableTempChat(ctx: ExecutionContext): Promise<void> {
    const resolver = ctx.selectors as SdkSelectorResolver;
    const toggle = await resolver.tryResolveLocator("temp_chat_toggle");
    if (!toggle) return;
    const pressed = await toggle.first().getAttribute("aria-pressed");
    if (pressed === "true") return;
    await ctx.pacing.beforeAction();
    await toggle.first().click();
  }

  // chat.continue — navigate the provider thread, divergence-check against the
  // mapping fingerprint, apply on_divergence before any submit. Returns an
  // AdapterEvent to emit when the task must not proceed, else null.
  private async prepareContinue(task: Task, ctx: ExecutionContext): Promise<AdapterEvent | null> {
    const providerThreadId =
      typeof task.options.provider_thread_id === "string"
        ? task.options.provider_thread_id
        : null;
    if (!providerThreadId) {
      return {
        t: "error",
        error: {
          class: "user_intervention_required",
          scope: "task",
          retryable: false,
          fallback_eligible: false,
          cooldown_s: null,
          user_action: "Pass options.provider_thread_id (from the thread mapping)",
          detail: "chat.continue requires a provider thread",
          evidence_ref: null,
        },
      };
    }
    const page = sdkPage(ctx.page);
    // §A6.5 — navigation stays inside manifest.origins (worker navlock).
    if (threadIdFromUrl(page.url(), THREAD_URL_PATTERN) !== providerThreadId) {
      await page.goto(`https://chatgpt.com/c/${providerThreadId}`);
    }
    await this.waitForThreadRender(ctx);
    const snapshot = await this.readThread(providerThreadId, ctx);
    const expected =
      typeof task.options.last_turn_fingerprint === "string"
        ? task.options.last_turn_fingerprint
        : null;
    const rawPolicy = task.options.on_divergence;
    const policy: DivergencePolicy =
      rawPolicy === "adopt" || rawPolicy === "fork" ? rawPolicy : "fail";
    const action = resolveDivergence(expected, snapshot.last_turn_fingerprint, policy);
    if (action === "fork") {
      return {
        t: "needs_user",
        reason: "confirm_dialog",
        message: "thread diverged from the mapping — fork into a new thread?",
      };
    }
    if (action === "fail") {
      return {
        t: "error",
        error: {
          class: "user_intervention_required",
          scope: "task",
          retryable: false,
          fallback_eligible: false,
          cooldown_s: null,
          user_action: "Review the provider thread; it diverged from the mapping",
          detail: "last_turn_fingerprint mismatch (on_divergence=fail)",
          evidence_ref: null,
        },
      };
    }
    return null;
  }

  // A navigated thread renders its turns after the load event (live: reading
  // it straight away found no "response"). Wait until assistant turns exist
  // and their count holds across two samples; readThread then reports what
  // the provider really shows (and throws — not_sent, retryable — if still
  // nothing after the cap).
  private async waitForThreadRender(ctx: ExecutionContext, capMs = 15000): Promise<void> {
    const page = sdkPage(ctx.page);
    const resolver = ctx.selectors as SdkSelectorResolver;
    let last = -1;
    for (let waited = 0; waited < capMs; waited += 250) {
      const turns = await resolver.tryResolveLocator("response");
      const count = turns ? await turns.count() : 0;
      if (count > 0 && count === last) return;
      last = count;
      await page.waitForTimeout(250);
    }
  }

  // image.generate — same composer, image entry point, captureImages → sink.
  private async *executeImage(task: Task, ctx: ExecutionContext): AsyncIterable<AdapterEvent> {
    const page = sdkPage(ctx.page);
    const resolver = ctx.selectors as SdkSelectorResolver;
    const cfg = this.cfg;
    const now = cfg.completion?.now ?? (() => Date.now());
    const sleep =
      cfg.completion?.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
    const pollIntervalMs = cfg.completion?.pollIntervalMs ?? 100;
    const timeoutMs = cfg.completion?.timeoutMs ?? 120000;
    const stallTimeoutS = cfg.stallTimeoutS ?? 90;
    const heartbeatIntervalMs = cfg.heartbeatIntervalMs ?? 15000;

    // Critical #5 — challenges halt, never retried.
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

    // Image generation is unavailable in temporary chats (provider rule), so
    // image tasks run in regular chats — organized by the image-chat policy:
    // the account's active image chat, else a new chat in the project.
    const reused = await this.openImageChat(task, ctx);
    // In a reused chat, everything already on the page belongs to earlier runs.
    const gallery0 = reused ? await resolver.tryResolveLocator("image_result") : null;
    if (gallery0) {
      await gallery0.locator("img").evaluateAll((els, attr) => {
        for (const el of els) el.setAttribute(attr, "1");
      }, SEEN_ATTR);
    }
    const NEW_IMG = `img:not([${SEEN_ATTR}])`;

    // §A3.4 — a vanished entry point is UI drift (§A9 fold: provider_ui_changed).
    if (!(await this.enableImageMode(ctx))) {
      yield {
        t: "error",
        error: {
          class: "provider_ui_changed",
          scope: "adapter",
          retryable: false,
          fallback_eligible: true,
          cooldown_s: null,
          user_action: null,
          detail: "image mode could not be enabled (composer_plus/image_menu_item/capability:image_tool_toggle)",
          evidence_ref: null,
        },
      };
      return;
    }

    await ctx.pacing.beforeTask();
    await fillComposer(page, resolver, task.prompt);
    await ctx.pacing.beforeAction();
    await ctx.markSubmitted(null); // §A1: sent_unconfirmed BEFORE Send
    await submit(page, resolver, { fallback: cfg.submitFallbackEnter ? "enter" : undefined });
    let threadId = threadIdFromUrl(page.url(), THREAD_URL_PATTERN);
    await ctx.markSubmitted(threadId); // §A1: acknowledged after provider ack
    const url = page.url();
    yield {
      t: "submitted",
      provider_thread_id: threadId,
      provider_url: url.startsWith("about:") ? null : url,
    };

    // D11 — heartbeat + partial image tiles while the provider renders.
    const tracker = createCompletionTracker(page, resolver, { ...cfg.completion, now, sleep });
    const pending: AdapterEvent[] = [];
    const hb = createHeartbeat(
      (e) => pending.push(e),
      heartbeatIntervalMs,
      { now, lastChangeAt: () => tracker.lastChangeAt() }
    );
    const startedAt = now();
    let lastHb = now();
    let seenTiles = 0;
    let imgSig = "";
    let imgSigAt = now();
    const imagesChangedRecently = (): boolean => now() - imgSigAt < stallTimeoutS * 1000;
    const imagesSettled = async (): Promise<boolean> => {
      const gallery = await resolver.tryResolveLocator("image_result");
      if (!gallery) return false;
      const state = await gallery
        .locator(NEW_IMG)
        .evaluateAll((els) =>
          els.map((el) => {
            const i = el as HTMLImageElement;
            return `${i.currentSrc || i.src}|${i.complete ? i.naturalWidth : 0}`;
          })
        );
      const sig = state.join(";");
      if (sig !== imgSig) {
        imgSig = sig;
        imgSigAt = now();
      }
      const loaded = state.length > 0 && state.every((x) => Number(x.split("|").pop()) > 0);
      return loaded && now() - imgSigAt >= (cfg.completion?.stabilityMs ?? 2000);
    };
    for (;;) {
      while (pending.length > 0) yield pending.shift() as AdapterEvent;
      // Same late thread-id capture as the SDK chat path.
      if (threadId === null) {
        threadId = threadIdFromUrl(page.url(), THREAD_URL_PATTERN);
        if (threadId !== null) await ctx.markSubmitted(threadId);
      }
      const { signals } = await tracker.pollOnce();
      // Image turns show no Stop button while rendering (text-side signals
      // fire early) and no Send button once done (the chat rule's send
      // signal never fires): finish on loaded, unchanged images with no
      // stop/streaming indicator and stable text.
      const imagesDone = await imagesSettled();
      if (imagesDone && signals.stop_absent && signals.streaming_absent && signals.stable) break;
      if (tracker.stalled(stallTimeoutS) && !imagesChangedRecently()) {
        yield {
          t: "error",
          error: stalledError(ctx.attempt.submission_state, `no DOM change for ${stallTimeoutS}s`),
        };
        return;
      }
      if (now() - startedAt >= timeoutMs) {
        yield {
          t: "error",
          error: timeoutError(ctx.attempt.submission_state, `no completion within ${timeoutMs}ms`),
        };
        return;
      }
      const container = await resolver.tryResolveLocator("image_result");
      const tiles = container ? await container.locator(NEW_IMG).count() : 0;
      if (tiles > seenTiles) {
        seenTiles = tiles;
        yield {
          t: "artifact.partial",
          ref: {
            provider: this.manifest.provider,
            provider_artifact_id: `partial-${tiles}`,
            provider_url: url.startsWith("about:") ? "about:blank" : url,
            provider_url_expires_at: null,
          },
        };
      }
      if (now() - lastHb >= heartbeatIntervalMs) {
        lastHb = now();
        hb.tick();
      }
      await sleep(pollIntervalMs);
    }
    while (pending.length > 0) yield pending.shift() as AdapterEvent;

    const result = await captureImages(page, resolver, ctx.artifacts, {
      provider: this.manifest.provider,
      allowedOrigins: this.manifest.origins,
      key: "image_result",
      imgSelector: NEW_IMG,
    });
    const container = await resolver.tryResolveLocator("image_result");
    const ids: Array<string | null> = container
      ? await container
          .locator(NEW_IMG)
          .evaluateAll((els) => els.map((el) => el.getAttribute("data-artifact-id")))
      : [];
    const chatUrl = page.url();
    for (const [i, f] of result.files.entries()) {
      yield {
        t: "artifact.ready",
        ref: {
          provider: this.manifest.provider,
          provider_artifact_id: ids[i] ?? f.sha256.slice(0, 16),
          provider_url: chatUrl.startsWith("about:") ? "about:blank" : chatUrl,
          provider_url_expires_at: null,
        },
        meta: { type: "image", mime_type: f.mime_type, format: f.format, title: task.prompt.slice(0, 120) },
      };
    }
    yield { t: "done", outcome: result.files.length > 0 ? "success" : "partial" };
  }

  // §A1/Critical #2 — reconcile: locate the thread, compare the last user
  // turn against prompt_fingerprint. Match → adopt; thread without match →
  // ambiguous; thread gone → not_found. Never resubmits.
  override async reconcile(
    attempt: TaskAttempt,
    ctx: ExecutionContext
  ): Promise<ReconcileResult> {
    const page = sdkPage(ctx.page);
    const resolver = createResolver(page, this.pack);
    if (
      attempt.provider_thread_id &&
      threadIdFromUrl(page.url(), THREAD_URL_PATTERN) !== attempt.provider_thread_id
    ) {
      await page.goto(`https://chatgpt.com/c/${attempt.provider_thread_id}`);
    }
    const userTurns = await resolver.tryResolveLocator("user_turn");
    const hasResponse = (await resolver.tryResolveLocator("response")) !== null;
    const count = userTurns ? await userTurns.count() : 0;
    if (count === 0 && !hasResponse) return { outcome: "not_found" };
    if (count === 0) {
      return { outcome: "ambiguous", detail: "response present but no user turn to fingerprint" };
    }
    const lastUserText = await userTurns!.nth(count - 1).innerText();
    if (userTurnFingerprint(lastUserText) === attempt.prompt_fingerprint) {
      return {
        outcome: "acknowledged",
        provider_thread_id: threadIdFromUrl(page.url(), THREAD_URL_PATTERN) ?? attempt.provider_thread_id,
      };
    }
    return {
      outcome: "ambiguous",
      detail: "thread exists but last user turn does not match prompt_fingerprint",
    };
  }
}

// Adapter-package convention (P3 activation): the gateway worker pool
// instantiates adapters through a zero-arg `createAdapter()` (or default
// export) found at <adapter dir>/adapter.ts.
export function createAdapter(): ChatGPTWebAdapter {
  return new ChatGPTWebAdapter();
}

export default createAdapter;
