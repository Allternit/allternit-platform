// Shared web-chat adapter base for provider chat UIs (claude-web, kimi-web):
// the SDK DeclarativeChatAdapter plus what every chat site needs around it —
// a fresh chat for chat.create, the mapped thread for chat.continue with the
// divergence check (Critical #7), announcement dialogs closed, fingerprint
// reconcile (Critical #2, never resubmits), and SingletonLock →
// profile_locked (fix #6). Provider specifics (URLs, selectors, banners)
// come from the adapter's config; nothing here names a provider.
//
// This directory has no manifest.yaml, so the registry never loads it as an
// adapter.
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
  createResolver,
  threadIdFromUrl,
  type DeclarativeChatConfig,
  type SdkPageLease,
  type SdkSelectorResolver,
} from "@allternit/subscription-adapter-sdk";

/** Buttons that close an informational dialog without accepting anything. */
const DISMISS_BUTTON = /^(close|dismiss|not now|maybe later|no thanks|skip|got it)$/i;

export function loadManifestAt(url: URL): AdapterManifest {
  return adapterManifestSchema.parse(yamlLoad(readFileSync(url, "utf8")));
}

// §A2/Critical #2 — fingerprint of a provider-side user turn; mirrors the
// gateway worker's promptFingerprint.
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

export function sdkPage(lease: ExecutionContext["page"]): Page {
  return (lease as SdkPageLease).page;
}

export interface WebChatSite {
  /** Where a new chat starts. */
  newChatUrl: string;
  /** A provider thread id → the URL that reopens it (inside manifest.origins). */
  threadUrl(providerThreadId: string): string;
}

export interface WebChatOptions {
  // chat.create starts from `site.newChatUrl` instead of whatever the lane
  // page shows (a previous task's thread). Default true; fixture tests that
  // load a page directly pass false.
  freshChat?: boolean;
}

export class WebChatAdapter extends DeclarativeChatAdapter {
  protected readonly cfg: DeclarativeChatConfig;

  constructor(
    protected readonly site: WebChatSite,
    config: DeclarativeChatConfig,
    protected readonly opts: WebChatOptions = {}
  ) {
    super(config);
    this.cfg = config;
  }

  override async *execute(task: Task, ctx: ExecutionContext): AsyncIterable<AdapterEvent> {
    try {
      if (task.capability === "chat.continue") {
        const gate = await this.prepareContinue(task, ctx);
        if (gate) {
          yield gate;
          return;
        }
      } else {
        // Never type into the page as left by the previous task: the prompt
        // would land in that task's thread.
        await this.openFreshChat(ctx);
      }
      await this.dismissAnnouncements(ctx);
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

  protected async openFreshChat(ctx: ExecutionContext): Promise<void> {
    if (this.opts.freshChat === false) return;
    await sdkPage(ctx.page).goto(this.site.newChatUrl, { waitUntil: "domcontentloaded" });
  }

  // Close informational dialogs over the composer. A dialog that asks for
  // input is never closed: that is a question for a person.
  protected async dismissAnnouncements(ctx: ExecutionContext): Promise<void> {
    const page = sdkPage(ctx.page);
    for (let round = 0; round < 3; round++) {
      const dialogs = page.locator('[role="dialog"], [role="alertdialog"]');
      let closed = false;
      for (let i = 0, n = await dialogs.count(); i < n; i++) {
        const dialog = dialogs.nth(i);
        if (!(await dialog.isVisible().catch(() => false))) continue;
        if (await dialog.locator('input, textarea, select, iframe, [contenteditable="true"]').count()) continue;
        const close = dialog.getByRole("button", { name: DISMISS_BUTTON }).first();
        if (!(await close.count())) continue;
        ctx.log.info("closing a provider announcement dialog");
        await ctx.pacing.beforeAction();
        await close.click({ timeout: 3000 }).catch(() => {});
        closed = true;
      }
      if (!closed) return;
      await page.waitForTimeout(300);
    }
  }

  protected async prepareContinue(task: Task, ctx: ExecutionContext): Promise<AdapterEvent | null> {
    const providerThreadId =
      typeof task.options.provider_thread_id === "string" ? task.options.provider_thread_id : null;
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
    if (threadIdFromUrl(page.url(), this.cfg.threadUrlPattern) !== providerThreadId) {
      await page.goto(this.site.threadUrl(providerThreadId));
    }
    await this.waitForThreadRender(ctx);
    const snapshot = await this.readThread(providerThreadId, ctx);
    const expected =
      typeof task.options.last_turn_fingerprint === "string" ? task.options.last_turn_fingerprint : null;
    const rawPolicy = task.options.on_divergence;
    const policy: DivergencePolicy = rawPolicy === "adopt" || rawPolicy === "fork" ? rawPolicy : "fail";
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

  // A navigated thread renders its turns after the load event. Wait until
  // assistant turns exist and their count holds across two samples.
  protected async waitForThreadRender(ctx: ExecutionContext, capMs = 15000): Promise<void> {
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

  override async reconcile(attempt: TaskAttempt, ctx: ExecutionContext): Promise<ReconcileResult> {
    const page = sdkPage(ctx.page);
    const resolver = createResolver(page, this.pack);
    const pattern = this.cfg.threadUrlPattern;
    if (attempt.provider_thread_id && threadIdFromUrl(page.url(), pattern) !== attempt.provider_thread_id) {
      await page.goto(this.site.threadUrl(attempt.provider_thread_id));
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
        provider_thread_id: threadIdFromUrl(page.url(), pattern) ?? attempt.provider_thread_id,
      };
    }
    return {
      outcome: "ambiguous",
      detail: "thread exists but last user turn does not match prompt_fingerprint",
    };
  }
}
