// chatgpt-web adapter: SDK conformance over the 6 canonical fixtures, execute
// e2e (temp-chat default, image capture into the real artifact store),
// divergence policy unit tests, reconcile outcome mapping, negative control.
// Fixtures only — no live provider contact.
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import type { Browser, Page } from "playwright";
import {
  ConformanceError,
  SelectorPack,
  createExecutionContext,
  createPacer,
  createPageLease,
  createResolver,
  runConformance,
} from "@allternit/subscription-adapter-sdk";
import type { AdapterEvent, Task, TaskAttempt } from "@allternit/subscription-fabric-contracts";
import { createArtifactStore } from "../src/artifacts/store.js";
import { openDatabase, type Db } from "../src/store/db.js";
import { getArtifact, listArtifactsForTask } from "../src/store/queries.js";
import {
  ChatGPTWebAdapter,
  isProfileLockError,
  resolveDivergence,
  THREAD_URL_PATTERN,
  userTurnFingerprint,
  type ChatGPTWebConfigOverrides,
} from "../adapters/chatgpt-web/adapter.js";
import { cleanupDir, launchBrowser, tmpStateDir } from "./helpers.js";

const FIXTURES_DIR = fileURLToPath(
  new URL("../adapters/chatgpt-web/fixtures/", import.meta.url)
);

const FAST: ChatGPTWebConfigOverrides = {
  completion: { stabilityMs: 150, pollIntervalMs: 25, timeoutMs: 5000 },
  heartbeatIntervalMs: 200,
  stallTimeoutS: 5,
};

let browser: Browser;
beforeAll(async () => {
  browser = await launchBrowser();
}, 30000);
afterAll(async () => {
  await browser.close();
}, 30000);

async function fixturePage(name: string): Promise<Page> {
  const page = await browser.newPage();
  const { readFileSync } = await import("node:fs");
  await page.setContent(readFileSync(join(FIXTURES_DIR, `${name}.html`), "utf8"));
  return page;
}

describe("conformance (canonical 6 states)", () => {
  it("chatgpt-web passes the shared suite against its fixtures", async () => {
    const report = await runConformance(
      () => {
        const adapter = new ChatGPTWebAdapter({ freshChat: false }, FAST);
        return {
          pack: adapter.pack,
          banners: [
            { kind: "limit_banner", pattern: /you'?ve reached (your )?(usage )?limit/i },
            { kind: "limit_banner", pattern: /approaching (your )?(usage )?limit/i },
            { kind: "slow_mode", pattern: /slower (responses|mode)|slow mode/i },
            { kind: "reset_notice", pattern: /(quota|limit|usage) resets? (at|in)/i },
          ],
          threadUrlPattern: /^https:\/\/chatgpt\.com\/c\/([\w-]+)/,
          sampleThreadUrl: "https://chatgpt.com/c/68f7c000-aaaa-bbbbbbbb",
          sampleThreadId: "68f7c000-aaaa-bbbbbbbb",
          probeInput: adapter.probeInput(),
          expectations: {
            idle: {
              resolves: [
                "composer",
                "send_button",
                "logged_in_probe",
                "temp_chat_toggle",
                "model_picker",
                "capability:image_tool_toggle",
              ],
              absent: ["stop_button", "streaming"],
              probeOk: true,
            },
          },
        };
      },
      FIXTURES_DIR,
      { browser }
    );
    expect(report.ok).toBe(true);
    expect(report.checks.length).toBeGreaterThan(10);
  }, 60000);

  it("negative control: a broken composer pack fails loudly, naming the key", async () => {
    const broken = SelectorPack.fromYaml(`
composer:
  critical: true
  strategies:
    - { testid: definitely-not-the-composer }
send_button:
  critical: true
  strategies:
    - { testid: send-button }
logged_in_probe:
  critical: true
  strategies:
    - { testid: profile-button }
banner:
  critical: false
  strategies:
    - { testid: limit-banner }
`);
    const err = await runConformance(
      () => ({
        pack: broken,
        banners: [],
        threadUrlPattern: /^https:\/\/chatgpt\.com\/c\/([\w-]+)/,
        sampleThreadUrl: "https://chatgpt.com/c/x-1",
        sampleThreadId: "x-1",
        probeInput: { auth: { login_url: "https://chatgpt.com/auth/login", logged_in_probe: "logged_in_probe" } },
      }),
      FIXTURES_DIR,
      { browser }
    ).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ConformanceError);
    expect((err as ConformanceError).message).toContain("composer");
  }, 60000);
});

describe("execute e2e against fixtures", () => {
  let dir: string;
  let db: Db;
  beforeEach(() => {
    dir = tmpStateDir();
    db = openDatabase(":memory:");
  });
  afterEach(() => {
    db.close();
    cleanupDir(dir);
  });

  function makeCtx(page: Page, adapter: ChatGPTWebAdapter, attempt: TaskAttempt) {
    const marks: string[] = [];
    const ctx = createExecutionContext({
      page: createPageLease(page),
      sink: createArtifactStore(db, {
        artifactsDir: join(dir, "artifacts"),
        source: {
          task_id: "task-cgw-1",
          attempt_no: 1,
          capability: "chat.create",
          provider: adapter.manifest.provider,
          account_id: "acct-1",
          adapter_id: adapter.manifest.adapter_id,
          adapter_version: adapter.manifest.adapter_version,
          thread_id: null,
          project_id: null,
          bot_id: null,
          sensitivity: "internal",
        },
      }),
      pacer: createPacer(
        { min_action_gap_ms: [1, 2], min_task_gap_s: 0, max_tasks_per_hour: 1000, max_tasks_per_day: 5000 },
        { rng: () => 0 }
      ),
      resolver: createResolver(page, adapter.pack),
      logger: { debug: () => {}, info: () => {}, warn: () => {}, error: () => {} },
      attempt,
      onMarkSubmitted: async (_t, state) => {
        marks.push(state);
      },
    });
    return { ctx, marks };
  }

  function makeAttempt(): TaskAttempt {
    return {
      attempt_no: 1,
      adapter_id: "chatgpt-web",
      adapter_version: "0.1.0",
      account_id: "acct-1",
      pool_key: "chatgpt:acct-1:chat-msgs",
      submission_state: "not_sent",
      prompt_fingerprint: "fp",
      provider_thread_id: null,
      requested_model_class: null,
      observed_model: null,
      started_at: new Date().toISOString(),
      ended_at: null,
      outcome: "failed",
      error: null,
    };
  }

  function makeTask(capability: Task["capability"], options: Record<string, unknown> = {}): Task {
    const now = new Date().toISOString();
    return {
      task_id: "task-cgw-1",
      idempotency_key: null,
      capability,
      capability_version: 1,
      requester: { kind: "user", id: "user-1" },
      initiated_by: { kind: "human", user_id: "user-1", action_id: "action-1" },
      thread_id: null,
      project_id: null,
      parent_task_id: null,
      prompt: "Summarize the migration plan.",
      inputs: [],
      options,
      routing: { mode: "auto", allow_fallback: true, allow_metered: false, allow_thread_migration: false },
      constraints: { sensitivity: "internal", deadline_at: null, max_metered_usd: null, required_export_format: null },
      approval_id: null,
      priority: "interactive",
      status: "queued",
      status_detail: null,
      route_decision: null,
      attempts: [],
      result: null,
      error: null,
      created_at: now,
      updated_at: now,
      completed_at: null,
    };
  }

  it("chat.create: temp-chat ON by default, submitted → reply → done", async () => {
    const adapter = new ChatGPTWebAdapter({ freshChat: false }, FAST);
    const page = await fixturePage("complete");
    const { ctx, marks } = makeCtx(page, adapter, makeAttempt());
    const events: AdapterEvent[] = [];
    for await (const e of adapter.execute(makeTask("chat.create"), ctx)) events.push(e);

    expect(await page.evaluate(() => document.body.dataset.tempChat)).toBe("on"); // D5
    expect(await page.evaluate(() => document.body.dataset.submitted)).toBe("true");
    expect(marks).toEqual(["sent_unconfirmed", "acknowledged"]);
    const kinds = events.map((e) => e.t);
    expect(kinds[0]).toBe("submitted");
    expect(kinds).toContain("reply");
    expect(kinds[kinds.length - 1]).toBe("done");
    const done = events[events.length - 1];
    expect(done.t === "done" && done.text).toBeTruthy();
    await page.close();
  }, 30000);

  it("chat.create: tempChat opt-out leaves the toggle untouched", async () => {
    const adapter = new ChatGPTWebAdapter({ tempChat: false, freshChat: false }, FAST);
    const page = await fixturePage("complete");
    const { ctx } = makeCtx(page, adapter, makeAttempt());
    for await (const _e of adapter.execute(makeTask("chat.create"), ctx)) {
      // drain
    }
    expect(await page.evaluate(() => document.body.dataset.tempChat)).toBeUndefined();
    await page.close();
  }, 30000);

  it("chat.create on a fabric thread stays out of temp chat (D5: temp is for stateless tasks only)", async () => {
    const adapter = new ChatGPTWebAdapter({ freshChat: false }, FAST);
    const page = await fixturePage("complete");
    const { ctx } = makeCtx(page, adapter, makeAttempt());
    const task = { ...makeTask("chat.create"), thread_id: "fab-1" };
    for await (const _e of adapter.execute(task, ctx)) {
      // drain
    }
    expect(await page.evaluate(() => document.body.dataset.tempChat)).toBeUndefined();
    expect(await page.evaluate(() => document.body.dataset.submitted)).toBe("true");
    await page.close();
  }, 30000);

  it("chat.create never types into the previous task's thread: it opens a fresh chat at the origin root first", async () => {
    const adapter = new ChatGPTWebAdapter({}, FAST);
    const page = await browser.newPage();
    const { readFileSync } = await import("node:fs");
    const html = readFileSync(join(FIXTURES_DIR, "complete.html"), "utf8");
    const served: string[] = [];
    await page.route("https://chatgpt.com/**", (route) => {
      served.push(route.request().url());
      return route.fulfill({ contentType: "text/html", body: html });
    });
    // The lane page as a prior chat.continue left it.
    await page.goto("https://chatgpt.com/c/mapped-thread-1");
    const { ctx } = makeCtx(page, adapter, makeAttempt());
    for await (const _e of adapter.execute(makeTask("chat.create"), ctx)) {
      // drain
    }
    expect(served).toEqual(["https://chatgpt.com/c/mapped-thread-1", "https://chatgpt.com/"]);
    expect(page.url()).toBe("https://chatgpt.com/");
    expect(await page.evaluate(() => document.body.dataset.submitted)).toBe("true");
    await page.close();
  }, 30000);

  it("chat.create closes an announcement dialog over the composer, then sends (live: 'Meet ChatGPT Work' stalled a task)", async () => {
    const adapter = new ChatGPTWebAdapter({ freshChat: false }, FAST);
    const page = await fixturePage("announcement");
    const { ctx } = makeCtx(page, adapter, makeAttempt());
    const events: AdapterEvent[] = [];
    for await (const e of adapter.execute(makeTask("chat.create"), ctx)) events.push(e);
    expect(await page.locator("#announce").count()).toBe(0);
    expect(await page.evaluate(() => document.body.dataset.submitted)).toBe("true");
    expect(events[events.length - 1].t).toBe("done");
    await page.close();
  }, 30000);

  it("a dialog that asks for input is never closed by the adapter", async () => {
    const adapter = new ChatGPTWebAdapter({ freshChat: false }, FAST);
    const page = await fixturePage("complete");
    await page.evaluate(() => {
      const d = document.createElement("div");
      d.setAttribute("role", "dialog");
      d.id = "asks";
      d.innerHTML = '<input aria-label="Email"><button type="button" aria-label="Close">×</button>';
      document.body.appendChild(d);
    });
    const { ctx } = makeCtx(page, adapter, makeAttempt());
    for await (const _e of adapter.execute(makeTask("chat.create"), ctx)) {
      // drain
    }
    expect(await page.locator("#asks").count()).toBe(1);
    await page.close();
  }, 30000);

  it("readAccount: the signed-in email and the sidebar's usage, never a token", async () => {
    const adapter = new ChatGPTWebAdapter({}, FAST);
    const page = await browser.newPage();
    const { readFileSync } = await import("node:fs");
    const html = readFileSync(join(FIXTURES_DIR, "idle.html"), "utf8").replace(
      "</body>",
      '<aside><span>8% usage remaining</span></aside></body>'
    );
    await page.route("https://chatgpt.com/**", (route) =>
      route.request().url().endsWith("/api/auth/session")
        ? route.fulfill({
            contentType: "application/json",
            body: JSON.stringify({ user: { email: "eoj@example.com", name: "Eoj" }, accessToken: "SECRET" }),
          })
        : route.fulfill({ contentType: "text/html", body: html })
    );
    await page.goto("https://chatgpt.com/");
    await adapter.attach({ adapter_id: "chatgpt-web", origins: [], navigate: async () => {}, page } as never);
    const seen = await adapter.readAccount(new AbortController().signal);
    expect(seen.identity).toBe("eoj@example.com");
    expect(seen.usage).toMatchObject({ remaining_pct: 8, resets_at: null });
    expect(JSON.stringify(seen)).not.toContain("SECRET");
    await page.close();
  }, 30000);

  it("chat.create on a challenge fixture → needs_user, no submit (Critical #5)", async () => {
    const adapter = new ChatGPTWebAdapter({ freshChat: false }, FAST);
    const page = await fixturePage("challenge");
    const { ctx } = makeCtx(page, adapter, makeAttempt());
    const events: AdapterEvent[] = [];
    for await (const e of adapter.execute(makeTask("chat.create"), ctx)) events.push(e);
    expect(events).toHaveLength(1);
    expect(events[0].t === "needs_user" && events[0].reason === "challenge").toBe(true);
    await page.close();
  }, 30000);

  it("image.generate: partial tiles → artifact.partial, completion → captureImages → artifact.ready into the real store", async () => {
    const adapter = new ChatGPTWebAdapter({ freshChat: false }, FAST);
    const page = await fixturePage("image-mid-run");
    const { ctx } = makeCtx(page, adapter, makeAttempt());

    // Drive the run in the background; complete the fixture shortly after.
    const events: AdapterEvent[] = [];
    const run = (async () => {
      for await (const e of adapter.execute(makeTask("image.generate"), ctx)) events.push(e);
    })();
    await page.waitForTimeout(300);
    await page.evaluate(() => {
      document.querySelector("[data-testid='stop-button']")?.remove();
      document.querySelector("[data-testid='send-button']")?.removeAttribute("disabled");
      document.querySelector(".result-streaming")?.classList.remove("result-streaming");
    });
    await run;

    const kinds = events.map((e) => e.t);
    expect(kinds[0]).toBe("submitted");
    expect(kinds).toContain("artifact.partial");
    expect(kinds.filter((k) => k === "artifact.ready")).toHaveLength(2);
    expect(kinds[kinds.length - 1]).toBe("done");

    const stored = listArtifactsForTask(db, "task-cgw-1");
    expect(stored).toHaveLength(2);
    for (const a of stored) {
      expect(a.storage.retrieval_state).toBe("local");
      expect(a.storage.sha256).toMatch(/^[0-9a-f]{64}$/);
      expect(a.storage.local_path).toBe(`${a.storage.sha256!.slice(0, 2)}/${a.storage.sha256}`);
      expect(getArtifact(db, a.artifact_id)?.type).toBe("image");
    }
    await page.close();
  }, 30000);

  it("image.generate against the live-UI shape: + menu → Create image chip → blob: gallery image → artifact.ready", async () => {
    const adapter = new ChatGPTWebAdapter({ freshChat: false }, FAST);
    const page = await browser.newPage();
    const PNG =
      "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
    await page.route("https://chatgpt.com/**", (route) =>
      route.fulfill({
        contentType: "text/html",
        body: `<main><div id="thread"></div>
          <button aria-label="Open profile menu">me</button>
          <div data-composer-body>
            <button aria-label="Add files and more" id="plus">+</button>
            <span id="chips"></span>
            <div role="textbox" aria-label="Ask ChatGPT" contenteditable="true"></div>
            <button aria-label="Send" type="submit" id="send">Send</button>
          </div>
          <div id="menu" hidden><div id="create">Create image</div></div></main>
          <script>
            plus.onclick = () => { menu.hidden = false; };
            create.onclick = () => { menu.hidden = true; chips.innerHTML = '<button aria-label="Remove Create image">Create image</button>'; };
            send.onclick = () => {
              thread.innerHTML = '<div data-user-message-bubble="true">p</div><div data-conversation-role="assistant"></div>';
              setTimeout(() => {
                const bytes = Uint8Array.from(atob("${PNG}"), (c) => c.charCodeAt(0));
                const src = URL.createObjectURL(new Blob([bytes], { type: "image/png" }));
                thread.querySelector("[data-conversation-role]").innerHTML =
                  '<div data-testid="generated-image-gallery"><button data-testid="generated-image-preview"><img alt="Generated image 1" src="' + src + '"></button></div>';
                send.remove(); // live UI: no Send button after an image reply
              }, 150);
            };
          </script>`,
      })
    );
    await page.goto("https://chatgpt.com/");
    const { ctx } = makeCtx(page, adapter, makeAttempt());
    const events: AdapterEvent[] = [];
    for await (const e of adapter.execute(makeTask("image.generate"), ctx)) events.push(e);
    const kinds = events.map((e) => e.t);
    expect(kinds).not.toContain("error");
    expect(await page.getByRole("button", { name: /remove create image/i }).count()).toBe(1);
    expect(kinds.filter((k) => k === "artifact.ready")).toHaveLength(1);
    expect(kinds[kinds.length - 1]).toBe("done");
    await page.close();
  }, 30000);

  // Live UI shape (2026-09-29): three hidden file inputs in the composer
  // form; uploading disables Send until the attachment chip lands.
  function attachApp(withInputs: boolean): string {
    const PNG =
      "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
    const inputs = withInputs
      ? `<input type="file" multiple accept="image/*,video/*" hidden id="f1">
         <input type="file" multiple accept="image/*" hidden id="f2">
         <input type="file" multiple hidden id="f3">`
      : "";
    return `<main><div id="thread"></div>
      <button aria-label="Open profile menu">me</button>
      <form data-composer-body>
        ${inputs}
        <button type="button" aria-label="Add files and more" id="plus">+</button>
        <span id="chips"></span><span id="files"></span>
        <div role="textbox" aria-label="Ask ChatGPT" contenteditable="true"></div>
        <button aria-label="Send" type="button" id="send">Send</button>
      </form>
      <div id="menu" hidden><div id="create">Create image</div></div></main>
      <script>
        plus.onclick = () => { menu.hidden = false; };
        create.onclick = () => { menu.hidden = true; chips.innerHTML = '<button type="button" aria-label="Remove Create image">Create image</button>'; };
        for (const input of document.querySelectorAll("input[type=file]")) {
          input.onchange = () => {
            send.disabled = true;
            setTimeout(() => {
              for (const f of input.files) files.insertAdjacentHTML("beforeend", '<span data-name="' + f.name + '" data-type="' + f.type + '" data-size="' + f.size + '" data-input="' + input.id + '"></span>');
              send.disabled = false;
            }, 200);
          };
        }
        send.onclick = () => {
          if (send.disabled) return;
          document.body.dataset.sentWith = String(files.children.length);
          thread.innerHTML = '<div data-user-message-bubble="true">p</div><div data-conversation-role="assistant"></div>';
          setTimeout(() => {
            const bytes = Uint8Array.from(atob("${PNG}"), (c) => c.charCodeAt(0));
            const src = URL.createObjectURL(new Blob([bytes], { type: "image/png" }));
            thread.querySelector("[data-conversation-role]").innerHTML =
              '<div data-testid="generated-image-gallery"><button data-testid="generated-image-preview"><img alt="Generated image 1" src="' + src + '"></button></div>';
            send.remove();
          }, 150);
        };
      </script>`;
  }
  const PHOTO = { type: "image" as const, mime_type: "image/jpeg" as const, data_base64: Buffer.from("jpeg-bytes").toString("base64") };

  it("image.generate with a reference photo: attaches via the image-only input, waits for upload, then sends", async () => {
    const adapter = new ChatGPTWebAdapter({ freshChat: false }, FAST);
    const page = await browser.newPage();
    await page.route("https://chatgpt.com/**", (route) => route.fulfill({ contentType: "text/html", body: attachApp(true) }));
    await page.goto("https://chatgpt.com/");
    const { ctx } = makeCtx(page, adapter, makeAttempt());
    const task = { ...makeTask("image.generate"), inputs: [PHOTO] };
    const events: AdapterEvent[] = [];
    for await (const e of adapter.execute(task, ctx)) events.push(e);
    const kinds = events.map((e) => e.t);
    expect(kinds).not.toContain("error");
    expect(kinds.filter((k) => k === "artifact.ready")).toHaveLength(1);
    const file = page.locator("#files span");
    expect(await file.getAttribute("data-input")).toBe("f2");
    expect(await file.getAttribute("data-type")).toBe("image/jpeg");
    expect(await file.getAttribute("data-name")).toBe("reference-1.jpg");
    expect(await file.getAttribute("data-size")).toBe("10");
    expect(await page.evaluate(() => document.body.dataset.sentWith)).toBe("1");
    await page.close();
  }, 30000);

  it("image.generate with a reference photo but no file input → provider_ui_changed, nothing sent", async () => {
    const adapter = new ChatGPTWebAdapter({ freshChat: false }, FAST);
    const page = await browser.newPage();
    await page.route("https://chatgpt.com/**", (route) => route.fulfill({ contentType: "text/html", body: attachApp(false) }));
    await page.goto("https://chatgpt.com/");
    const { ctx } = makeCtx(page, adapter, makeAttempt());
    const events: AdapterEvent[] = [];
    for await (const e of adapter.execute({ ...makeTask("image.generate"), inputs: [PHOTO] }, ctx)) events.push(e);
    const last = events[events.length - 1];
    expect(last.t === "error" && last.error.class === "provider_ui_changed").toBe(true);
    expect(await page.evaluate(() => document.body.dataset.sentWith)).toBeUndefined();
    await page.close();
  }, 30000);

  // Live UI shape (2026-09-28) for the image-chat policy: sidebar Projects
  // section, composer "+" → "Create image", images as blob: gallery tiles.
  const GEN_PNG =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
  const OLD_PNG =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";
  // lateMs: the sidebar and earlier turns render that long after load (live).
  function imageApp(opts: { oldImage?: boolean; projects?: "link" | "create"; lateMs?: number }): string {
    const projects =
      opts.projects === "link"
        ? // live: project entries are buttons (not links) with nested actions
          `<section data-app-action-sidebar-section-heading="Projects"><ul><li>
             <div role="button" aria-label="Allternit">Allternit
               <button aria-label="Project actions for Allternit">…</button>
               <button aria-label="New chat in Allternit" onclick="history.pushState({}, '', '/g/g-p-abc-allternit/project')">+</button>
             </div></li></ul></section>`
        : opts.projects === "create"
          ? // Live: the create button sits in a zero-width wrapper that only
            // expands while the section TITLE row is hovered.
            // (live geometry, 2026-09-28: unhovered, the button sits at x=350,
            // outside the 330px-wide sidebar; title hover brings it to x=298).
            `<style>.title{position:relative;width:330px;height:30px}
               .reveal{position:absolute;left:350px;top:0}
               .title:hover .reveal{left:298px}</style>
             <section data-app-action-sidebar-section-heading="Projects" style="padding:40px 0;width:338px;overflow:hidden">
               <div class="title"><button data-app-action-sidebar-section-toggle>Projects</button>
               <span class="reveal"><button data-app-action-sidebar-project-create aria-label="Add new project" id="np">+</button></span></div>
               <div style="height:400px">No projects</div></section>`
          : "";
    const old = opts.oldImage
      ? `<div data-user-message-bubble="true">old prompt</div><div data-conversation-role="assistant">
           <div data-testid="generated-image-gallery"><button data-testid="generated-image-preview">
           <img alt="old" src="data:image/png;base64,${OLD_PNG}"></button></div></div>`
      : "";
    const late = opts.lateMs
      ? `<script>setTimeout(() => {
           document.querySelector("nav").innerHTML = ${JSON.stringify(projects)};
           thread.insertAdjacentHTML("afterbegin", ${JSON.stringify(old)});
           const np2 = document.getElementById("np"); if (np2) np2.onclick = window.__openDialog;
         }, ${opts.lateMs});</script>`
      : "";
    return `<nav>${opts.lateMs ? "" : projects}</nav><div id="dlg"></div><main><div id="thread">${opts.lateMs ? "" : old}</div>
      <button aria-label="Open profile menu">me</button>
      <div data-composer-body>
<span id="plusSlot"></span><span id="chips"></span>
        <div role="textbox" aria-label="Ask ChatGPT" contenteditable="true"></div>
        <button aria-label="Send" type="submit" id="send">Send</button>
      </div>
</main>
      <script>
        const np = document.getElementById("np");
        window.__openDialog = () => {
          dlg.innerHTML = '<div role="dialog" aria-label="Create project"><input aria-label="Project name"><button id="cp">Create project</button></div>';
          cp.onclick = () => {
            window.__created = dlg.querySelector("input").value;
            dlg.innerHTML = "";
            history.pushState({}, "", "/g/g-p-abc-allternit/project");
          };
        };
        if (np) np.onclick = window.__openDialog;
        // live: the composer "+" renders after the textbox, and its menu is
        // only added to the DOM a beat after the click
        let plus;
        setTimeout(() => {
          plusSlot.innerHTML = '<button aria-label="Add files and more" id="plus">+</button>';
          plus = document.getElementById("plus");
          plus.onclick = openMenu;
        }, 700);
        const openMenu = () => setTimeout(() => {
          document.querySelector("main").insertAdjacentHTML("beforeend", '<div id="menu"><div id="create">Create image</div></div>');
          document.getElementById("create").onclick = () => {
            document.getElementById("menu").remove();
            chips.innerHTML = '<button aria-label="Remove Create image">Create image</button>';
          };
        }, 300);
        send.onclick = () => {
          thread.insertAdjacentHTML("beforeend", '<div data-user-message-bubble="true">p</div><div data-conversation-role="assistant"></div>');
          setTimeout(() => {
            const bytes = Uint8Array.from(atob("${GEN_PNG}"), (c) => c.charCodeAt(0));
            const src = URL.createObjectURL(new Blob([bytes], { type: "image/png" }));
            const turns = thread.querySelectorAll("[data-conversation-role]");
            turns[turns.length - 1].innerHTML =
              '<div data-testid="generated-image-gallery"><button data-testid="generated-image-preview"><img alt="Generated image 1" src="' + src + '"></button></div>';
            chips.innerHTML = "";
          }, 400);
        };
      </script>${late}`;
  }
  async function imagePage(html: string, gone?: string): Promise<Page> {
    const page = await browser.newPage();
    await page.route("https://chatgpt.com/**", (route) =>
      route.fulfill({
        contentType: "text/html",
        body:
          gone && route.request().url().endsWith(gone)
            ? `<script>history.replaceState({}, "", "/");</script>${html}`
            : html,
      })
    );
    return page;
  }
  async function runImageTask(page: Page, options: Record<string, unknown>) {
    // The lane page is already on the provider when a task starts.
    await page.goto("https://chatgpt.com/");
    const adapter = new ChatGPTWebAdapter({}, FAST);
    const { ctx } = makeCtx(page, adapter, makeAttempt());
    const events: AdapterEvent[] = [];
    for await (const e of adapter.execute(makeTask("image.generate", options), ctx)) events.push(e);
    return events;
  }

  it("image chat reuse: captures only this run's image, not the chat's earlier ones", async () => {
    const page = await imagePage(imageApp({ oldImage: true }));
    const events = await runImageTask(page, { image_chat_url: "https://chatgpt.com/c/img-chat-1" });
    const kinds = events.map((e) => e.t);
    expect(kinds).not.toContain("error");
    expect(page.url()).toBe("https://chatgpt.com/c/img-chat-1");
    expect(kinds.filter((k) => k === "artifact.ready")).toHaveLength(1);
    // The captured image is this run's (GEN_PNG), not the chat's earlier one.
    const { createHash } = await import("node:crypto");
    const genSha = createHash("sha256").update(Buffer.from(GEN_PNG, "base64")).digest("hex");
    const ready = events.find((e) => e.t === "artifact.ready");
    expect(ready && ready.t === "artifact.ready" && ready.ref.provider_artifact_id).toBe(genSha.slice(0, 16));
    await page.close();
  }, 30000);

  it("image project missing → created via the sidebar dialog, image runs in it", async () => {
    const page = await imagePage(imageApp({ projects: "create" }));
    const events = await runImageTask(page, { image_project: "Allternit" });
    expect(events.map((e) => e.t)).not.toContain("error");
    expect(await page.evaluate(() => (window as unknown as { __created?: string }).__created)).toBe("Allternit");
    expect(page.url()).toBe("https://chatgpt.com/g/g-p-abc-allternit/project");
    expect(events.filter((e) => e.t === "artifact.ready")).toHaveLength(1);
    await page.close();
  }, 30000);

  it("image project present → opened from the sidebar link", async () => {
    const page = await imagePage(imageApp({ projects: "link" }));
    const events = await runImageTask(page, { image_project: "Allternit" });
    expect(events.map((e) => e.t)).not.toContain("error");
    expect(page.url()).toBe("https://chatgpt.com/g/g-p-abc-allternit/project");
    await page.close();
  }, 30000);

  it("image chat gone → falls back to a new chat in the project", async () => {
    const page = await imagePage(imageApp({ projects: "link" }), "/c/deleted-chat");
    const events = await runImageTask(page, {
      image_chat_url: "https://chatgpt.com/c/deleted-chat",
      image_project: "Allternit",
    });
    expect(events.map((e) => e.t)).not.toContain("error");
    expect(page.url()).toBe("https://chatgpt.com/g/g-p-abc-allternit/project");
    expect(events.filter((e) => e.t === "artifact.ready")).toHaveLength(1);
    await page.close();
  }, 30000);

  it("image chat reuse waits for LATE-rendering earlier images before marking them (live bug)", async () => {
    const page = await imagePage(imageApp({ oldImage: true, lateMs: 1200 }));
    const events = await runImageTask(page, { image_chat_url: "https://chatgpt.com/c/img-chat-late" });
    expect(events.map((e) => e.t)).not.toContain("error");
    const { createHash } = await import("node:crypto");
    const genSha = createHash("sha256").update(Buffer.from(GEN_PNG, "base64")).digest("hex");
    const ready = events.filter((e) => e.t === "artifact.ready");
    expect(ready).toHaveLength(1);
    expect(ready[0].t === "artifact.ready" && ready[0].ref.provider_artifact_id).toBe(genSha.slice(0, 16));
    await page.close();
  }, 30000);

  it("image project: waits for a LATE sidebar and opens the existing project (live bug)", async () => {
    const page = await imagePage(imageApp({ projects: "link", lateMs: 1500 }));
    const events = await runImageTask(page, { image_project: "Allternit" });
    expect(events.map((e) => e.t)).not.toContain("error");
    expect(page.url()).toBe("https://chatgpt.com/g/g-p-abc-allternit/project");
    await page.close();
  }, 30000);

  it("image project URL known → goes straight to the project page", async () => {
    const page = await imagePage(imageApp({}));
    const events = await runImageTask(page, {
      image_project: "Allternit",
      image_project_url: "https://chatgpt.com/g/g-p-abc-allternit/project",
    });
    expect(events.map((e) => e.t)).not.toContain("error");
    expect(page.url()).toBe("https://chatgpt.com/g/g-p-abc-allternit/project");
    const ready = events.find((e) => e.t === "artifact.ready");
    // artifact refs carry the chat URL as shown at capture time
    expect(ready && ready.t === "artifact.ready" && ready.ref.provider_url).toBe(page.url());
    await page.close();
  }, 30000);

  it("THREAD_URL_PATTERN reads project chat URLs", () => {
    expect(THREAD_URL_PATTERN.exec("https://chatgpt.com/g/g-p-abc-allternit/c/6aba-1")?.[1]).toBe("6aba-1");
    expect(THREAD_URL_PATTERN.exec("https://chatgpt.com/c/6aba-2")?.[1]).toBe("6aba-2");
    expect(THREAD_URL_PATTERN.exec("https://chatgpt.com/g/g-p-abc-allternit/project")).toBeNull();
  });

  it("chat.continue waits for a navigated thread to render before the divergence read", async () => {
    const adapter = new ChatGPTWebAdapter({ freshChat: false }, FAST);
    const { readFileSync } = await import("node:fs");
    const completeHtml = readFileSync(join(FIXTURES_DIR, "complete.html"), "utf8");
    const probe = await fixturePage("complete");
    const { ctx: probeCtx } = makeCtx(probe, adapter, makeAttempt());
    const snapshot = await adapter.readThread("thread-late", probeCtx);
    await probe.close();

    // Live shape: the load event fires before the SPA renders the turns.
    const page = await browser.newPage();
    await page.route("https://chatgpt.com/**", (route) =>
      route.fulfill({
        contentType: "text/html",
        body: `<body></body><script>setTimeout(() => {
          document.open(); document.write(${JSON.stringify(completeHtml).replace(/<\//g, "<\\/")}); document.close();
        }, 600);</script>`,
      })
    );
    const { ctx } = makeCtx(page, adapter, makeAttempt());
    const events: AdapterEvent[] = [];
    for await (const e of adapter.execute(
      makeTask("chat.continue", {
        provider_thread_id: "thread-late",
        last_turn_fingerprint: snapshot.last_turn_fingerprint,
      }),
      ctx
    )) events.push(e);
    expect(events.map((e) => e.t)).not.toContain("error");
    expect(events[events.length - 1].t).toBe("done");
    await page.close();
  }, 30000);

  it("chat.continue: fingerprint match proceeds; mismatch with fail policy errors; fork asks", async () => {
    const adapter = new ChatGPTWebAdapter({ freshChat: false }, FAST);
    const { readFileSync } = await import("node:fs");
    const completeHtml = readFileSync(join(FIXTURES_DIR, "complete.html"), "utf8");
    // Offline navigation: page.goto("https://chatgpt.com/c/<id>") is fulfilled
    // locally — no live provider contact.
    const routeThread = async (page: Page) => {
      await page.route("https://chatgpt.com/**", (route) =>
        route.fulfill({ body: completeHtml, contentType: "text/html" })
      );
    };

    // learn the fixture's true assistant-turn fingerprint via readThread
    const page1 = await fixturePage("complete");
    const { ctx: ctx1 } = makeCtx(page1, adapter, makeAttempt());
    const snapshot = await adapter.readThread("thread-1", ctx1);

    // missing provider_thread_id → error guard, nothing submitted
    const events: AdapterEvent[] = [];
    for await (const e of adapter.execute(
      makeTask("chat.continue", { provider_thread_id: null }),
      ctx1
    )) events.push(e);
    expect(events[0].t).toBe("error");

    // match → proceed (full declarative flow runs and completes)
    const page2 = await fixturePage("complete");
    await routeThread(page2);
    const { ctx: ctx2 } = makeCtx(page2, adapter, makeAttempt());
    const okEvents: AdapterEvent[] = [];
    for await (const e of adapter.execute(
      makeTask("chat.continue", {
        provider_thread_id: "thread-1",
        last_turn_fingerprint: snapshot.last_turn_fingerprint,
      }),
      ctx2
    )) okEvents.push(e);
    expect(okEvents.map((e) => e.t)).toContain("done");
    expect(page2.url()).toBe("https://chatgpt.com/c/thread-1");

    // mismatch + fail → error, no submit
    const page3 = await fixturePage("complete");
    await routeThread(page3);
    const { ctx: ctx3 } = makeCtx(page3, adapter, makeAttempt());
    const failEvents: AdapterEvent[] = [];
    for await (const e of adapter.execute(
      makeTask("chat.continue", {
        provider_thread_id: "thread-1",
        last_turn_fingerprint: "deadbeef",
        on_divergence: "fail",
      }),
      ctx3
    )) failEvents.push(e);
    expect(failEvents).toHaveLength(1);
    expect(failEvents[0].t).toBe("error");
    expect(await page3.evaluate(() => document.body.dataset.submitted)).toBeUndefined();

    // mismatch + fork → needs_user confirm
    const page4 = await fixturePage("complete");
    await routeThread(page4);
    const { ctx: ctx4 } = makeCtx(page4, adapter, makeAttempt());
    const forkEvents: AdapterEvent[] = [];
    for await (const e of adapter.execute(
      makeTask("chat.continue", {
        provider_thread_id: "thread-1",
        last_turn_fingerprint: "deadbeef",
        on_divergence: "fork",
      }),
      ctx4
    )) forkEvents.push(e);
    expect(forkEvents[0].t === "needs_user" && forkEvents[0].reason === "confirm_dialog").toBe(true);

    await Promise.all([page1.close(), page2.close(), page3.close(), page4.close()]);
  }, 60000);
});

describe("divergence policy (pure)", () => {
  it("match proceeds; mismatch applies on_divergence", () => {
    expect(resolveDivergence("abc", "abc", "fail")).toBe("proceed");
    expect(resolveDivergence(null, "abc", "fail")).toBe("proceed"); // no baseline → proceed
    expect(resolveDivergence("abc", "xyz", "adopt")).toBe("proceed");
    expect(resolveDivergence("abc", "xyz", "fork")).toBe("fork");
    expect(resolveDivergence("abc", "xyz", "fail")).toBe("fail");
  });

  it("userTurnFingerprint normalizes whitespace and matches the worker's prompt-only fingerprint", () => {
    expect(userTurnFingerprint("hello   world\n")).toBe(userTurnFingerprint("hello world"));
  });

  it("profile-lock classifier (fix #6)", () => {
    expect(isProfileLockError(new Error("SingletonLock: cannot access profile"))).toBe(true);
    expect(isProfileLockError(new Error("user data directory is already in use"))).toBe(true);
    expect(isProfileLockError(new Error("net::ERR_FAILED"))).toBe(false);
  });
});

describe("reconcile outcome mapping (Critical #2)", () => {
  function attemptWith(fp: string, threadId: string | null): TaskAttempt {
    return {
      attempt_no: 1,
      adapter_id: "chatgpt-web",
      adapter_version: "0.1.0",
      account_id: "acct-1",
      pool_key: "chatgpt:acct-1:chat-msgs",
      submission_state: "sent_unconfirmed",
      prompt_fingerprint: fp,
      provider_thread_id: threadId,
      requested_model_class: null,
      observed_model: null,
      started_at: new Date().toISOString(),
      ended_at: null,
      outcome: "failed",
      error: null,
    };
  }

  function reconcileCtx(page: Page, adapter: ChatGPTWebAdapter) {
    return createExecutionContext({
      page: createPageLease(page),
      sink: {
        begin: async () => "a",
        write: async () => {},
        commit: async () => {},
        fail: async () => {},
      },
      pacer: createPacer(
        { min_action_gap_ms: [1, 2], min_task_gap_s: 0, max_tasks_per_hour: 1, max_tasks_per_day: 1 },
        { rng: () => 0 }
      ),
      resolver: createResolver(page, adapter.pack),
      logger: { debug: () => {}, info: () => {}, warn: () => {}, error: () => {} },
      attempt: attemptWith("fp", null),
      onMarkSubmitted: async () => {},
    });
  }

  it("last user turn matches fingerprint → acknowledged (adopt)", async () => {
    const adapter = new ChatGPTWebAdapter({ freshChat: false }, FAST);
    const page = await fixturePage("complete");
    const fp = userTurnFingerprint("Summarize the migration plan.");
    const res = await adapter.reconcile(attemptWith(fp, null), reconcileCtx(page, adapter));
    expect(res.outcome).toBe("acknowledged");
    await page.close();
  }, 30000);

  it("thread exists but last user turn differs → ambiguous", async () => {
    const adapter = new ChatGPTWebAdapter({ freshChat: false }, FAST);
    const page = await fixturePage("complete");
    const res = await adapter.reconcile(attemptWith("deadbeef", null), reconcileCtx(page, adapter));
    expect(res.outcome).toBe("ambiguous");
    await page.close();
  }, 30000);

  it("thread gone → not_found", async () => {
    const adapter = new ChatGPTWebAdapter({ freshChat: false }, FAST);
    const page = await fixturePage("logged-out");
    const res = await adapter.reconcile(attemptWith("deadbeef", null), reconcileCtx(page, adapter));
    expect(res.outcome).toBe("not_found");
    await page.close();
  }, 30000);
});

describe("chatgpt-web THREAD_URL_PATTERN (live UI, P3 gate)", () => {
  it("ignores the provisional /c/local-… id and matches the server id", async () => {
    const { THREAD_URL_PATTERN } = await import("../adapters/chatgpt-web/adapter.js");
    const { threadIdFromUrl } = await import("@allternit/subscription-adapter-sdk");
    expect(threadIdFromUrl("https://chatgpt.com/c/local-chatgpt:abc?temporary-chat=true", THREAD_URL_PATTERN)).toBeNull();
    expect(
      threadIdFromUrl("https://chatgpt.com/c/6ab9e2e2-cdbc-83ea-92e3-91624aaa1e57?temporary-chat=true", THREAD_URL_PATTERN)
    ).toBe("6ab9e2e2-cdbc-83ea-92e3-91624aaa1e57");
  });
});
