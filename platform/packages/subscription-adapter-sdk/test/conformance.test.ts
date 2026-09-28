import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { fileURLToPath } from "node:url";
import type { Browser } from "playwright";
import {
  ConformanceError,
  DeclarativeChatAdapter,
  runConformance,
  type ConformanceTarget,
} from "../src/index";
import { launchBrowser } from "./helpers";
import { fixtureWebConfig } from "./fixture-web";

const FIXTURES_DIR = fileURLToPath(new URL("./fixtures", import.meta.url));

let browser: Browser;
beforeAll(async () => {
  browser = await launchBrowser();
}, 30000);
afterAll(async () => {
  await browser.close();
}, 30000);

function targetFrom(config = fixtureWebConfig()): ConformanceTarget {
  const adapter = new DeclarativeChatAdapter(config);
  return {
    pack: adapter.pack,
    banners: config.banners,
    threadUrlPattern: config.threadUrlPattern,
    sampleThreadUrl: config.sampleThreadUrl ?? "",
    sampleThreadId: config.sampleThreadId ?? "",
    probeInput: adapter.probeInput(),
  };
}

describe("conformance suite (§A3.5)", () => {
  it("fixture-web passes the full suite", async () => {
    const report = await runConformance(() => targetFrom(), FIXTURES_DIR, { browser });
    expect(report.ok).toBe(true);
    expect(report.failures).toEqual([]);
    expect(report.checks.length).toBeGreaterThan(10);
    const threadIdCheck = report.checks.find((c) => c.check === "thread-id");
    expect(threadIdCheck?.ok).toBe(true);
  }, 30000);

  // P2 gate — the suite fails loudly on a deliberately-broken selector.
  it("negative: a broken critical selector fails loudly naming the key", async () => {
    const broken = fixtureWebConfig({
      selectorsYaml: fixtureWebConfig()
        .selectorsYaml.replace(
          /- \{ role: textbox, name: \/\^message fixture-web\$\/i \}/,
          "- { testid: fw-composer-renamed-away }"
        )
        .replace("- { testid: fw-composer }", "- { testid: fw-composer-also-gone }")
        .replace('- { css: "div#fw-composer" }', '- { css: "div#fw-composer-gone" }'),
    });
    const err = await runConformance(() => targetFrom(broken), FIXTURES_DIR, {
      browser,
    }).catch((e) => e);
    expect(err).toBeInstanceOf(ConformanceError);
    const report = (err as ConformanceError).report;
    expect(report.ok).toBe(false);
    const composerFailures = report.failures.filter((f) => f.check.includes("composer"));
    expect(composerFailures.length).toBeGreaterThan(0);
    expect(err.message).toContain("composer");
  }, 30000);
});
