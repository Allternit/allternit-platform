// gemini-spark-web adapter — Gemini Spark through the Gemini web app (route 2,
// browser sign-in). It reuses GeminiWebAdapter (same composer, reply
// extraction, readAccount and banners) and changes only the entry point: a
// fresh task opens gemini.google.com/app and selects the side panel's "Spark"
// tab first. Spark is a beta task agent (cloud VMs), so chat.continue on a
// Spark thread is best-effort until a live thread shape is confirmed.
// v1-unverified: the tab locator below is an INFERRED guess (see README).
import { readFileSync } from "node:fs";
import type { DeclarativeChatConfig } from "@allternit/subscription-adapter-sdk";
import type { ExecutionContext, Task } from "@allternit/subscription-fabric-contracts";
import { GeminiWebAdapter } from "../gemini-web/adapter.js";
import { loadManifestAt, sdkPage, type WebChatOptions } from "../_shared/web-chat.js";

const GEMINI_APP = "https://gemini.google.com/app";

export function geminiSparkConfig(overrides: Partial<DeclarativeChatConfig> = {}): Partial<DeclarativeChatConfig> {
  return {
    manifest: loadManifestAt(new URL("./manifest.yaml", import.meta.url)),
    selectorsYaml: readFileSync(new URL("./selectors/v1.yaml", import.meta.url), "utf8"),
    ...overrides,
  };
}

export class GeminiSparkWebAdapter extends GeminiWebAdapter {
  constructor(opts: WebChatOptions = {}, configOverrides: Partial<DeclarativeChatConfig> = {}) {
    super(opts, geminiSparkConfig(configOverrides));
  }

  // Entry point override: open the Gemini app, then the side panel's Spark tab.
  // If the tab is missing (not yet rolled out to this account, or a region
  // without Spark) this throws, so the task fails instead of silently running
  // as an ordinary Gemini chat.
  protected override async openFreshChat(ctx: ExecutionContext, _task?: Task): Promise<void> {
    if (this.opts.freshChat === false) return;
    const page = sdkPage(ctx.page);
    await page.goto(GEMINI_APP, { waitUntil: "domcontentloaded" });
    // INFERRED (v1-unverified): Spark is a tab next to "Chat" in the side panel.
    const tab = page.getByRole("tab", { name: /^spark$/i }).first();
    if ((await tab.count()) === 0) {
      throw new Error("Gemini Spark tab not found: Spark may not be available on this Google account yet");
    }
    await tab.click();
  }
}

export function createAdapter(): GeminiSparkWebAdapter {
  return new GeminiSparkWebAdapter();
}

export default createAdapter;
