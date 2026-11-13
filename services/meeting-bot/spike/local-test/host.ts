// Test-call host for the spike (runs on the Mac, headless). It never touches
// Eoj's own Chrome: it has its own profile, signed in once with `--login`
// (a visible window; copied cookies don't work because Google signs out a
// copied session). Then it creates a Meet with meet.new, joins as host, admits guests, then speaks a
// WAV through the same media override the bot uses and posts a chat line.
//
//   npx tsx local-test/host.ts --out out/host --admit 2 --speak host.wav \
//     --chat "Gizzi, what did we decide?" --minutes 6
//
// Writes <out>/meet-url.txt as soon as the meeting exists, and
// <out>/host-events.jsonl + host-*.png at each step.
import { chromium, type Page } from "playwright-core";
import { appendFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import path from "node:path";
import { installGizziMedia } from "../gizzi-media.js";

const argv = process.argv.slice(2);
const get = (f: string) => { const i = argv.indexOf(f); return i >= 0 ? argv[i + 1] : undefined; };
const OUT = get("--out") ?? "out/host";
const ADMIT = Number(get("--admit") ?? "1");
const SPEAK = get("--speak");
const CHAT = get("--chat");
const MINUTES = Number(get("--minutes") ?? "6");
const CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const PROFILE = path.join(homedir(), "Library/Application Support/allternit-meet-host-chrome");
const LOGIN = argv.includes("--login"); // headed: a person signs in to Google once; the profile is kept for later runs

mkdirSync(OUT, { recursive: true });
const log = (event: string, data?: unknown) => {
  const line = JSON.stringify({ t: Date.now(), event, data });
  appendFileSync(path.join(OUT, "host-events.jsonl"), line + "\n");
  console.log(line);
};
let shot = 0;
const snap = async (page: Page, tag: string) => {
  await page.screenshot({ path: path.join(OUT, `host-${String(++shot).padStart(2, "0")}-${tag}.png`) }).catch(() => {});
};

async function clickIf(page: Page, name: RegExp, timeout = 1500): Promise<boolean> {
  const b = page.getByRole("button", { name }).first();
  try {
    await b.waitFor({ state: "visible", timeout });
    await b.click();
    return true;
  } catch {
    return false;
  }
}

async function main() {
  // Copied cookies are signed out by Google (device-bound session), so the host keeps its own signed-in profile.
  mkdirSync(PROFILE, { recursive: true });
  const ctx = await chromium.launchPersistentContext(PROFILE, {
    executablePath: CHROME,
    headless: !LOGIN,
    // Playwright adds --use-mock-keychain on macOS, which hides the real Keychain key that decrypts the copied cookies.
    ignoreDefaultArgs: ["--use-mock-keychain"],
    viewport: { width: 1280, height: 800 },
    args: ["--autoplay-policy=no-user-gesture-required", "--use-fake-ui-for-media-stream", "--no-first-run"],
    permissions: ["camera", "microphone"],
  });
  await ctx.addInitScript(installGizziMedia, { name: "Test host", state: "Host" });
  const page = ctx.pages()[0] ?? (await ctx.newPage());
  if (LOGIN) {
    await page.goto("https://accounts.google.com/ServiceLogin?continue=https://meet.google.com/", { waitUntil: "domcontentloaded" });
    log("login_window_open");
    await page.waitForURL(/^https:\/\/meet\.google\.com\//, { timeout: 10 * 60_000 });
    await page.waitForTimeout(3000);
    log("login_done", page.url());
    await ctx.close();
    return 0;
  }
  try {
    await page.goto("https://meet.new", { waitUntil: "domcontentloaded", timeout: 60_000 });
    await page.waitForURL(/meet\.google\.com\/[a-z]{3}-[a-z]{4}-[a-z]{3}/, { timeout: 60_000 }).catch(() => {});
    const m = page.url().match(/https:\/\/meet\.google\.com\/[a-z]{3}-[a-z]{4}-[a-z]{3}/);
    await snap(page, "prejoin");
    if (!m) { log("no_meeting_url", page.url()); return 3; }
    writeFileSync(path.join(OUT, "meet-url.txt"), m[0] + "\n");
    log("meeting_created", m[0]);

    if (!(await clickIf(page, /^(Join now|Start meeting)$/i, 30_000))) { await snap(page, "no-join"); log("join_button_missing"); return 4; }
    log("host_joined");
    await page.waitForTimeout(4000);
    await clickIf(page, /^(Got it|Dismiss|Close)$/i);
    await snap(page, "in-call");

    let admitted = 0;
    let spoke = false;
    const end = Date.now() + MINUTES * 60_000;
    while (Date.now() < end) {
      // Meet shows "Admit" on a toast, or "View all" -> per-person "Admit" in a dialog.
      if (await clickIf(page, /^Admit( \d+ guests?| all)?$/i, 500)) {
        admitted++;
        log("admitted", admitted);
        await page.waitForTimeout(800);
        await clickIf(page, /^(Admit|Admit all)$/i, 500); // confirm dialog, if any
        await snap(page, `admitted-${admitted}`);
      } else if (await clickIf(page, /^View all$/i, 300)) {
        await page.waitForTimeout(500);
      }
      if (!spoke && admitted >= ADMIT) {
        spoke = true;
        await page.waitForTimeout(3000);
        if (SPEAK) {
          const b64 = readFileSync(SPEAK).toString("base64");
          page.evaluate((u) => window.__gizziSpeak?.(u), `data:audio/wav;base64,${b64}`)
            .then((r) => log("host_spoke_done", r)).catch((e) => log("host_speak_error", String(e)));
          log("host_speaking");
        }
        if (CHAT) {
          await page.waitForTimeout(15_000);
          await clickIf(page, /^Chat with everyone/i, 2000);
          const box = page.getByRole("textbox", { name: /Send a message/i }).first();
          try {
            await box.fill(CHAT, { timeout: 5000 });
            await box.press("Enter");
            log("host_chat_sent", CHAT);
          } catch (e) { log("host_chat_failed", String(e)); }
          await snap(page, "chat");
        }
      }
      await page.waitForTimeout(700);
    }
    await snap(page, "end");
    await clickIf(page, /^Leave call$/i, 2000);
    log("host_left");
    return 0;
  } finally {
    await ctx.close().catch(() => {});
  }
}

main().then((c) => process.exit(c ?? 0), (e) => { log("fatal", String(e)); process.exit(1); });
