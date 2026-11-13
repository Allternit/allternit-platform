// Step 0.3 spike: one headed Chrome joins a Google Meet call as a guest.
//
//   node dist/join.js --url https://meet.google.com/abc-defg-hij \
//     --name "Gizzi (Allternit)" --minutes 30 [--out out/run1] [--speak hello.wav]
//
// Camera and mic are ours (gizzi-media.ts): the camera is our real bot, the
// allternit-ai /meeting-tile page (Bot Mode stage), opened in a second window
// and screencast into the camera track (tile-feed.ts); the mic is a WebAudio
// track. --tile-url points at the page (default: the local dev server).
// Without a tile (page unreachable, or --no-tile) the camera stays off and
// Meet shows its own initial for the bot.
//
// Control on stdin (one per line): joining | listening | thinking | speaking |
// paused  (the tile's state), or  speak <file.wav>. Call audio leaves Chrome through the default
// PulseAudio sink (meet_out, see setup-linux.sh), where capture.sh records it.
//
// Writes into --out:
//   events.jsonl        state changes and errors (prejoin, waiting_admit, live, denied, timeout, guest_blocked, ended, left),
//                       plus peer_track_added / peer_track_ended for every remote audio track
//   call.webm           the call audio from the peer connections (opus; CAPTURE_PEER=0 turns it off)
//   speakers.jsonl      {t, name} for every speaking participant, polled every 250 ms (t = epoch ms)
//   participants.jsonl  {t, participants} whenever the set changes
//   chat.jsonl          {t, id, sender, text} for every new chat message
//   state-*.png         a screenshot at each state change
//   tile-sample.html    one participant tile's DOM, 30 s into the call, for fixing selectors

import { chromium, type Browser, type BrowserContext, type Page } from "playwright-core";
import { spawn, type ChildProcess } from "node:child_process";
import { appendFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createInterface } from "node:readline";
import { installGizziMedia } from "./gizzi-media.js";
import { drainPeerCapture, installPeerCapture } from "./peer-capture.js";
import { startTileFeed, TILE_STATES, type TileFeed, type TileState } from "./tile-feed.js";
import { meet, rx, type TextMatch } from "./meet-selectors.js";

// ---- args ---------------------------------------------------------------------

interface Args {
  url: string;
  name: string;
  minutes: number;
  out: string;
  chrome: string;
  speak?: string;
  pageUrl: string;
  headless: boolean;
  keepProfile: boolean;
  profile?: string;
  tileUrl: string;
  agent?: string;
  tile: boolean;
}

function defaultChrome(): string {
  if (process.env.CHROME_PATH) return process.env.CHROME_PATH;
  if (process.platform === "darwin") return "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
  return "/usr/bin/google-chrome";
}

function parseArgs(argv: string[]): Args {
  const get = (flag: string): string | undefined => {
    const i = argv.indexOf(flag);
    return i >= 0 ? argv[i + 1] : undefined;
  };
  const url = get("--url");
  if (!url || !/^https:\/\/meet\.google\.com\//.test(url)) {
    console.error('usage: join --url https://meet.google.com/xxx-xxxx-xxx [--name "Gizzi (Allternit)"] [--minutes 30] [--out dir] [--speak file.wav]');
    process.exit(2);
  }
  const minutes = Number(get("--minutes") ?? "30");
  if (!Number.isFinite(minutes) || minutes <= 0) {
    console.error("--minutes must be a positive number");
    process.exit(2);
  }
  return {
    url,
    name: get("--name") ?? "Gizzi (Allternit)",
    minutes,
    out: resolve(get("--out") ?? join("out", new Date().toISOString().replace(/[:.]/g, "-"))),
    chrome: get("--chrome") ?? defaultChrome(),
    speak: get("--speak"),
    pageUrl: get("--page-url") ?? "<placeholder>",
    headless: argv.includes("--headless"),
    keepProfile: argv.includes("--keep-profile") || argv.includes("--profile"),
    // A signed-in bot profile (free-Gmail meetings refuse signed-out guests; spike result 2). Never deleted.
    profile: get("--profile"),
    // The bot's camera picture. Production: https://ai.allternit.com/meeting-tile
    tileUrl: get("--tile-url") ?? process.env.TILE_URL ?? "http://127.0.0.1:3013/meeting-tile",
    // base64 JSON of the user's own bot (allternit-ai Agent); absent = Gizzi.
    agent: get("--agent"),
    tile: !argv.includes("--no-tile"),
  };
}

// ---- timing constants (plan: admit timeout 10 min, leave when alone 5 min) ----

const POLL_MS = 250;
const CHAT_POLL_MS = 1000;
const PREJOIN_TIMEOUT_MS = 90_000;
const ADMIT_TIMEOUT_MS = 10 * 60_000;
const ALONE_LEAVE_MS = 5 * 60_000;
const ENDED_CONFIRM_CHECKS = 4; // leave button missing on 4 checks in a row (1 s apart)

type State = "launching" | "prejoin" | "waiting_admit" | "live" | "denied" | "guest_blocked" | "timeout" | "ended" | "left" | "error";

// ---- small helpers ------------------------------------------------------------

const args = parseArgs(process.argv.slice(2));
mkdirSync(args.out, { recursive: true });

function jsonl(file: string, row: unknown): void {
  appendFileSync(join(args.out, file), JSON.stringify(row) + "\n");
}

let state: State = "launching";
let tile: TileFeed | null = null;
/** What the tile shows for each call state (the rest leave it as it is). */
const TILE_FOR: Partial<Record<State, TileState>> = { launching: "joining", prejoin: "joining", waiting_admit: "joining", live: "listening" };

async function setState(page: Page | null, next: State, detail?: unknown): Promise<void> {
  if (next === state) return;
  state = next;
  const t = TILE_FOR[next];
  if (t && tile) await tile.setState(t);
  const row = { t: Date.now(), state: next, ...(detail === undefined ? {} : { detail }) };
  jsonl("events.jsonl", row);
  console.log(`[join] ${new Date(row.t).toISOString()} ${next}${detail === undefined ? "" : " " + JSON.stringify(detail)}`);
  if (page && !page.isClosed()) {
    await page.screenshot({ path: join(args.out, `state-${next}.png`) }).catch(() => undefined);
  }
}

function logEvent(kind: string, detail?: unknown): void {
  const row = { t: Date.now(), event: kind, ...(detail === undefined ? {} : { detail }) };
  jsonl("events.jsonl", row);
  console.log(`[join] ${kind}${detail === undefined ? "" : " " + JSON.stringify(detail)}`);
}

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

async function visibleButton(page: Page, m: TextMatch) {
  const b = page.getByRole("button", { name: rx(m) }).first();
  return (await b.isVisible().catch(() => false)) ? b : null;
}

async function clickIfVisible(page: Page, m: TextMatch): Promise<boolean> {
  const b = await visibleButton(page, m);
  if (!b) return false;
  await b.click({ timeout: 3000 }).catch(() => undefined);
  return true;
}

async function textVisible(page: Page, m: TextMatch): Promise<boolean> {
  return page.getByText(rx(m)).first().isVisible().catch(() => false);
}

// ---- in-call reads (run in the page) ------------------------------------------

interface CallSnapshot {
  participants: string[];
  speaking: string[];
}

async function snapshot(page: Page): Promise<CallSnapshot> {
  return page.evaluate(
    ({ tileCss, nameCss, speakCss }) => {
      const byId = new Map<string, { name: string; speaking: boolean }>();
      for (const tile of Array.from(document.querySelectorAll<HTMLElement>(tileCss))) {
        const id = tile.getAttribute("data-participant-id") ?? "";
        let name = "";
        for (const el of Array.from(tile.querySelectorAll<HTMLElement>(nameCss))) {
          const txt = (el.getAttribute("data-self-name") ?? el.textContent ?? "").trim();
          if (txt && txt.length < 80) {
            name = txt;
            break;
          }
        }
        const speaking = tile.querySelector(speakCss) !== null;
        const prev = byId.get(id);
        // A participant can have two tiles (camera + screen share): merge.
        byId.set(id, { name: prev?.name || name || id, speaking: Boolean(prev?.speaking) || speaking });
      }
      const all = Array.from(byId.values());
      return {
        participants: all.map((p) => p.name).sort(),
        speaking: all.filter((p) => p.speaking).map((p) => p.name),
      };
    },
    { tileCss: meet.participantTile.css, nameCss: meet.tileName.css, speakCss: meet.speakingClasses.css },
  );
}

interface ChatRow {
  id: string;
  sender: string;
  text: string;
}

async function readChat(page: Page): Promise<ChatRow[]> {
  return page.evaluate(
    ({ msgCss, senderCss }) =>
      Array.from(document.querySelectorAll<HTMLElement>(msgCss)).map((el) => {
        const group = el.closest<HTMLElement>(senderCss);
        const sender =
          group?.getAttribute("data-sender-name") ??
          (group?.firstElementChild as HTMLElement | null)?.innerText.split("\n")[0] ??
          "";
        return { id: el.getAttribute("data-message-id") ?? "", sender: sender.trim(), text: el.innerText.trim() };
      }),
    { msgCss: meet.chatMessage.css, senderCss: meet.chatSenderGroup.css },
  );
}

async function openChat(page: Page): Promise<boolean> {
  const input = page.locator(meet.chatInput.css).first();
  if (await input.isVisible().catch(() => false)) return true;
  await clickIfVisible(page, meet.chatButton);
  return input.waitFor({ state: "visible", timeout: 5000 }).then(
    () => true,
    () => false,
  );
}

async function postChat(page: Page, text: string): Promise<boolean> {
  if (!(await openChat(page))) return false;
  const input = page.locator(meet.chatInput.css).first();
  await input.fill(text);
  if (!(await clickIfVisible(page, meet.chatSend))) await input.press("Enter");
  return true;
}

// ---- the run ----------------------------------------------------------------

async function prejoin(page: Page): Promise<boolean> {
  await setState(page, "prejoin");
  const deadline = Date.now() + PREJOIN_TIMEOUT_MS;
  let named = false;
  while (Date.now() < deadline) {
    if (await textVisible(page, meet.guestBlockedText)) {
      await setState(page, "guest_blocked");
      return false;
    }
    await clickIfVisible(page, meet.dismissSignIn);
    await clickIfVisible(page, meet.continueWithoutDevices);
    const nameInput = page.locator(meet.nameInput.css).first();
    if (!named && (await nameInput.isVisible().catch(() => false))) {
      await nameInput.fill(args.name);
      named = true;
      logEvent("name_filled", args.name);
    }
    // Our devices are fake and harmless, so the mic goes on, and the camera
    // too when it has the tile to show (else Meet shows its own initial).
    if (await clickIfVisible(page, meet.micOn)) logEvent("mic_on");
    if (tile && (await clickIfVisible(page, meet.camOn))) logEvent("cam_on");
    const join = await visibleButton(page, meet.joinButton);
    if (join && (named || !(await nameInput.isVisible().catch(() => false)))) {
      await join.click({ timeout: 5000 });
      logEvent("join_clicked");
      return true;
    }
    await sleep(500);
  }
  await setState(page, "timeout", { phase: "prejoin" });
  return false;
}

async function waitForAdmit(page: Page): Promise<boolean> {
  await setState(page, "waiting_admit");
  const deadline = Date.now() + ADMIT_TIMEOUT_MS;
  while (Date.now() < deadline) {
    if (await visibleButton(page, meet.leaveButton)) {
      // "Leave call" also shows in the waiting room on some builds; live means
      // the waiting copy is gone too.
      if (!(await textVisible(page, meet.waitingText))) return true;
    }
    if (await textVisible(page, meet.deniedText)) {
      await setState(page, "denied");
      return false;
    }
    if (await textVisible(page, meet.guestBlockedText)) {
      await setState(page, "guest_blocked");
      return false;
    }
    await sleep(1000);
  }
  await setState(page, "timeout", { phase: "admit", minutes: ADMIT_TIMEOUT_MS / 60_000 });
  return false;
}

// Call recording (no PulseAudio needed): peer-capture.ts, an init script,
// mixes every remote WebRTC audio track into one MediaRecorder (webm/opus,
// 1 s chunks). Node pulls the chunks into <out>/call.webm about once a second
// and once more with a flush when leaving, and logs per-track added/ended
// events into events.jsonl. On by default; CAPTURE_PEER=0 turns it off (then
// only capture.sh's PulseAudio recording remains).
const CAPTURE_PEER = process.env.CAPTURE_PEER !== "0";
const CAPTURE_PULL_MS = 1000;
let captureChain: Promise<unknown> = Promise.resolve();
let captureBytes = 0;

function pullCapture(page: Page, flush = false): Promise<unknown> {
  if (!CAPTURE_PEER) return Promise.resolve();
  // Serialised, so chunks are appended in the order the page produced them.
  captureChain = captureChain.then(async () => {
    captureBytes += await drainPeerCapture(page, args.out, (e) => {
      jsonl("events.jsonl", e);
      console.log(`[join] ${e.event}${e.detail === undefined ? "" : " " + JSON.stringify(e.detail)}`);
    }, { flush });
  }).catch(() => undefined);
  return captureChain;
}

// Voice reply: the tile shows speaking (swelling with the voice level) while
// the audio plays into the mic, then goes back to listening.
async function speak(page: Page, file: string): Promise<void> {
  const b64 = readFileSync(file).toString("base64");
  await tile?.setState("speaking");
  const levels = setInterval(() => {
    void page
      .evaluate(() => window.__gizziOutLevel?.() ?? 0)
      .then((v) => tile?.setLevel(v))
      .catch(() => undefined);
  }, 80);
  const r = await page
    .evaluate((u) => window.__gizziSpeak?.(u), `data:audio/wav;base64,${b64}`)
    .catch((e: unknown) => ({ error: String(e) }));
  clearInterval(levels);
  tile?.setLevel(0);
  if (state === "live") await tile?.setState("listening");
  logEvent("spoke", r);
}

// The hook the agent turn uses until the runner (step 2.1) drives this in
// process: "thinking" when a turn starts, "speak <file>" for a voice reply,
// "listening" when the turn ends, "paused" / "listening" for stop/resume.
function listenForControl(page: Page): void {
  const rl = createInterface({ input: process.stdin });
  rl.on("line", (line) => {
    const [cmd, ...rest] = line.trim().split(/\s+/);
    if (!cmd) return;
    if ((TILE_STATES as readonly string[]).includes(cmd)) {
      void tile?.setState(cmd as TileState);
    } else if (cmd === "speak" && rest[0]) {
      void speak(page, rest.join(" "));
    } else {
      logEvent("control_unknown", line.slice(0, 120));
    }
  });
}

async function live(page: Page): Promise<void> {
  await setState(page, "live");

  const notice = `${args.name} joined to take notes. Transcribing this call. Meeting page: ${args.pageUrl}`;
  logEvent(await postChat(page, notice) ? "notice_posted" : "notice_failed", notice);

  if (args.speak) await speak(page, args.speak);
  listenForControl(page);

  const endAt = Date.now() + args.minutes * 60_000;
  let aloneSince: number | null = null;
  let lastParticipants = "";
  let missingLeave = 0;
  let lastChat = 0;
  let lastLeaveCheck = 0;
  let lastPull = 0;
  let sampled = false;
  const liveAt = Date.now();
  const seenChat = new Set<string>();

  while (true) {
    const now = Date.now();
    if (now >= endAt) {
      await pullCapture(page, true);
      logEvent("leave_reason", "time_limit");
      return;
    }

    if (now - lastPull >= CAPTURE_PULL_MS) {
      lastPull = now;
      void pullCapture(page);
    }
    const snap = await snapshot(page).catch(() => null);
    if (snap) {
      for (const name of snap.speaking) jsonl("speakers.jsonl", { t: now, name });
      const key = snap.participants.join("\u0000");
      if (key !== lastParticipants) {
        lastParticipants = key;
        jsonl("participants.jsonl", { t: now, participants: snap.participants });
      }
      // Only our own tile left (or none rendered) counts as alone.
      if (snap.participants.length <= 1) {
        aloneSince ??= now;
        if (now - aloneSince >= ALONE_LEAVE_MS) {
          logEvent("leave_reason", "alone_5_min");
          return;
        }
      } else {
        aloneSince = null;
      }
    }

    if (now - lastChat >= CHAT_POLL_MS) {
      lastChat = now;
      for (const m of await readChat(page).catch(() => [] as ChatRow[])) {
        const id = m.id || `${m.sender}:${m.text}`;
        if (seenChat.has(id)) continue;
        seenChat.add(id);
        if (m.text === notice || m.sender === "You") continue;
        jsonl("chat.jsonl", { t: now, id: m.id, sender: m.sender, text: m.text });
      }
    }

    if (now - lastLeaveCheck >= 1000) {
      lastLeaveCheck = now;
      if (await visibleButton(page, meet.leaveButton)) {
        missingLeave = 0;
      } else if (++missingLeave >= ENDED_CONFIRM_CHECKS) {
        await setState(page, (await textVisible(page, meet.deniedText)) ? "denied" : "ended", { reason: "removed_or_ended" });
        return;
      }
    }

    if (!sampled && now - liveAt > 30_000) {
      sampled = true;
      const html = await page
        .evaluate((css) => {
          const tiles = Array.from(document.querySelectorAll<HTMLElement>(css));
          const other = tiles.find((t) => !t.querySelector("[data-self-name]")) ?? tiles[0];
          return other ? other.outerHTML.slice(0, 40_000) : "";
        }, meet.participantTile.css)
        .catch(() => "");
      writeFileSync(join(args.out, "tile-sample.html"), html);
    }

    await sleep(Math.max(0, POLL_MS - (Date.now() - now)));
  }
}

async function main(): Promise<void> {
  const profile = args.profile ?? mkdtempSync(join(tmpdir(), "gizzi-meet-profile-"));
  const chromeArgs = [
    "--autoplay-policy=no-user-gesture-required", // our AudioContext and the call audio play without a click
    "--use-fake-ui-for-media-stream", // no permission bubble (our getUserMedia never asks, this covers Meet's own checks)
    "--disable-background-timer-throttling",
    "--disable-renderer-backgrounding",
    "--disable-backgrounding-occluded-windows", // the tile window keeps animating behind the meeting window
    "--force-color-profile=srgb", // tile frames keep their colours on wide-gamut (P3) Mac displays
    "--window-size=1280,800",
  ];
  if (process.getuid?.() === 0) chromeArgs.push("--no-sandbox"); // the pod runs as root, like allternit-headless.service

  logEvent("launch", { chrome: args.chrome, profile, out: args.out, minutes: args.minutes });
  let chrome: ChildProcess | null = null;
  let browser: Browser | null = null;
  let context: BrowserContext | null = null;
  let page: Page | null = null;
  let stopping = false;
  const stop = async (why: string) => {
    if (stopping) return;
    stopping = true;
    logEvent("leave_reason", why);
    if (page && !page.isClosed()) {
      // Before leaving: the recorder's partial chunk and any track events
      // (a signal exit used to drop everything since the last pull).
      await pullCapture(page, true);
      if (CAPTURE_PEER) logEvent("capture_stats", { file: "call.webm", bytes: captureBytes });
      await clickIfVisible(page, meet.leaveButton);
      await sleep(1500);
    }
    if (tile) logEvent("tile_stats", tile.stats());
    await tile?.stop();
    await browser?.close().catch(() => undefined);
    chrome?.kill("SIGTERM");
    await sleep(500);
    if (!args.keepProfile) rmSync(profile, { recursive: true, force: true });
  };
  process.on("SIGINT", () => void stop("sigint").then(() => process.exit(130)));
  process.on("SIGTERM", () => void stop("sigterm").then(() => process.exit(143)));

  try {
    // Meet refuses browsers in automation mode ("confirm you're not a bot" ->
    // "You can't join this video call"; spike result 1 in SPEC-meetings). So
    // Chrome starts as a normal browser (no --enable-automation) and we drive
    // it over its DevTools port. The bot still joins under its own name, with
    // its own status tile, and announces itself in chat. Eoj's call, 11-12.
    chrome = spawn(args.chrome, [
      ...chromeArgs,
      `--user-data-dir=${profile}`,
      "--remote-debugging-port=0",
      "--no-first-run",
      "--no-default-browser-check",
      "--lang=en-US",
      ...(args.headless ? ["--headless=new"] : []),
      ...(process.env.CHROME_EXTRA_ARGS ? process.env.CHROME_EXTRA_ARGS.split(" ") : []),
      "about:blank",
    ], { stdio: "ignore" });
    const portFile = join(profile, "DevToolsActivePort");
    for (let i = 0; i < 100 && !existsSync(portFile); i++) await sleep(100);
    const port = readFileSync(portFile, "utf8").split("\n")[0].trim();
    browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
    context = browser.contexts()[0];
    await context.grantPermissions(["camera", "microphone"], { origin: "https://meet.google.com" });
    if (process.env.NO_MEDIA_OVERRIDE !== "1") await context.addInitScript(installGizziMedia, { name: args.name, state: "Joining" });
    if (CAPTURE_PEER) await context.addInitScript(installPeerCapture, { timesliceMs: 1000 });
    page = context.pages()[0] ?? (await context.newPage());
    page.on("pageerror", (e) => logEvent("pageerror", String(e).slice(0, 300)));

    if (args.tile && process.env.NO_MEDIA_OVERRIDE !== "1") {
      tile = await startTileFeed(browser, page, {
        url: args.tileUrl,
        name: args.name,
        state: "joining",
        agent: args.agent,
        onEvent: logEvent,
      }).catch((e: unknown) => {
        logEvent("tile_unavailable", { url: args.tileUrl, error: String(e).slice(0, 300) });
        return null;
      });
    }

    await page.goto(args.url, { waitUntil: "domcontentloaded", timeout: 60_000 });
    if (!(await prejoin(page))) return;
    if (!(await waitForAdmit(page))) return;
    await live(page);
    await setState(page, "left");
  } catch (e) {
    await setState(page, "error", String(e).slice(0, 500));
    process.exitCode = 1;
  } finally {
    if (["denied", "guest_blocked", "timeout"].includes(state)) process.exitCode = 3;
    await stop(`final_state:${state}`);
  }
}

void main();
