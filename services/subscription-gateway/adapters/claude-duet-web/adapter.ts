// claude-duet-web adapter — Claude Duet through claude.ai on the web (route 2,
// browser sign-in). It reuses ClaudeWebAdapter (same composer, reply
// extraction, readAccount and banners) and changes only the entry point: Duet
// keeps all history in one thread, so a task opens Duet's thread instead of a
// fresh Claude chat. Duet works on its own cloud computer and messages back
// later, so a reply may arrive long after the send (best-effort).
// v1-unverified: the entry locator below is an INFERRED guess (see README).
import { readFileSync } from "node:fs";
import type { DeclarativeChatConfig } from "@allternit/subscription-adapter-sdk";
import type { ExecutionContext, Task } from "@allternit/subscription-fabric-contracts";
import { ClaudeWebAdapter } from "../claude-web/adapter.js";
import { loadManifestAt, sdkPage, type WebChatOptions } from "../_shared/web-chat.js";
import { adapterFile } from "../_shared/adapter-file.js";

const CLAUDE_APP = "https://claude.ai/";

export function claudeDuetConfig(overrides: Partial<DeclarativeChatConfig> = {}): Partial<DeclarativeChatConfig> {
  return {
    manifest: loadManifestAt(adapterFile(import.meta.url, "claude-duet-web", "./manifest.yaml")),
    selectorsYaml: readFileSync(adapterFile(import.meta.url, "claude-duet-web", "./selectors/v1.yaml"), "utf8"),
    ...overrides,
  };
}

export class ClaudeDuetWebAdapter extends ClaudeWebAdapter {
  constructor(opts: WebChatOptions = {}, configOverrides: Partial<DeclarativeChatConfig> = {}) {
    super(opts, claudeDuetConfig(configOverrides));
  }

  // Entry point override: open claude.ai, then Duet. If Duet is missing (still
  // on the waitlist, not Pro/Max, or outside the US) this throws, so the task
  // fails instead of silently running as an ordinary Claude chat.
  protected override async openFreshChat(ctx: ExecutionContext, _task?: Task): Promise<void> {
    if (this.opts.freshChat === false) return;
    const page = sdkPage(ctx.page);
    await page.goto(CLAUDE_APP, { waitUntil: "domcontentloaded" });
    // INFERRED (v1-unverified): Duet is a named link or tab in the claude.ai sidebar.
    const entry = page.getByRole("link", { name: /^(claude )?duet$/i }).or(page.getByRole("tab", { name: /^(claude )?duet$/i })).first();
    if ((await entry.count()) === 0) {
      throw new Error("Claude Duet not found: this Claude account may not have Duet yet (waitlist, Pro/Max in the US)");
    }
    await entry.click();
  }
}

export function createAdapter(): ClaudeDuetWebAdapter {
  return new ClaudeDuetWebAdapter();
}

export default createAdapter;
