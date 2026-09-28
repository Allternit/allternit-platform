import { afterAll, beforeAll, describe, expect, it } from "vitest";
import type { Browser } from "playwright";
import { DeclarativeChatAdapter, SelectorPack, createResolver, probe } from "../src/index";
import { fixtureHtml, fixturePage, launchBrowser, loadPack } from "./helpers";
import { FIXTURE_WEB_EXTRA_SELECTORS, fixtureWebConfig } from "./fixture-web";

let browser: Browser;
beforeAll(async () => {
  browser = await launchBrowser();
}, 30000);
afterAll(async () => {
  await browser.close();
}, 30000);

const probeInput = {
  auth: { login_url: "https://fixture-web.test/login", logged_in_probe: "logged_in_probe" },
  criticalKeys: ["composer", "send_button", "logged_in_probe"],
};

describe("probe (§A3.4)", () => {
  it("idle → ok with auth + critical checks passing", async () => {
    const page = await fixturePage(browser, "idle.html");
    const result = await probe(page, createResolver(page, loadPack()), loadPack(), probeInput);
    expect(result.ok).toBe(true);
    const auth = result.checks.find((c) => c.key === "auth.state");
    expect(auth).toMatchObject({ critical: true, ok: true, detail: "ready" });
    await page.close();
  });

  it("logged-out → ok:false with auth and probe-locator checks failing", async () => {
    const page = await fixturePage(browser, "logged-out.html");
    const result = await probe(page, createResolver(page, loadPack()), loadPack(), probeInput);
    expect(result.ok).toBe(false);
    const failing = result.checks.filter((c) => !c.ok).map((c) => c.key);
    expect(failing).toContain("auth.state");
    expect(failing).toContain("logged_in_probe");
    expect(result.checks.find((c) => c.key === "auth.state")?.detail).toBe("auth_required");
    await page.close();
  });

  it("capability entry-point locators from the pack get non-critical checks", async () => {
    const pack = SelectorPack.fromYaml(fixtureHtml("selectors.v1.yaml") + FIXTURE_WEB_EXTRA_SELECTORS);
    const page = await fixturePage(browser, "idle.html");
    const result = await probe(page, createResolver(page, pack), pack, probeInput);
    const cap = result.checks.find((c) => c.key === "capability:chat");
    expect(cap).toMatchObject({ critical: false, ok: true });
    await page.close();
  });

  it("DeclarativeChatAdapter.probe works over the attached runtime page", async () => {
    const config = fixtureWebConfig();
    const adapter = new DeclarativeChatAdapter(config);
    const page = await fixturePage(browser, "idle.html");
    await adapter.attach({
      adapter_id: "fixture-web",
      origins: ["https://fixture-web.test"],
      navigate: async () => {},
      page,
    });
    const result = await adapter.probe(new AbortController().signal);
    expect(result.ok).toBe(true);
    await adapter.detach();
    await page.close();
  });
});
