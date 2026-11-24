// Allternit Meetings lane 2.4: Microsoft Teams adapter, anonymous web join.
// Same pattern as spike/join.ts (Meet): a normal Chrome (no --enable-automation)
// driven over its DevTools port. Every selector lives in teams-selectors.ts.
//
// Flow: open the link -> "Continue on this browser" -> name -> mic/cam -> "Join now"
// -> lobby (waitForAdmit) -> live. Chat is best effort: anonymous users have no
// chat in some tenants, and postChat then resolves false (CONTRACT).
//
// Media: the camera/mic are the pod's media override. The caller passes
// `installMedia` (the spike's installGizziMedia wrapper) and `setTile`; without them
// Chrome's fake devices are used so pre-join still works.

import type { Browser, BrowserContext, Page } from "playwright-core";
import { launchChrome } from "../chrome.js";
import type { ChildProcess } from "node:child_process";
import { mkdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type {
  ActiveSpeakerEvent,
  AdapterFactory,
  AdmitResult,
  CaptionEvent,
  ChatMessage,
  EndReason,
  JoinPhase,
  JoinRequest,
  JoinResult,
  Participant,
  PlatformAdapter,
  PodConfig,
  TileState,
  Unsubscribe,
} from "../adapter.js";
import { rx, teams, type TextMatch } from "./teams-selectors.js";

export interface TeamsAdapterOptions {
  chromePath?: string;
  headless?: boolean;
  /** Chrome profile dir; a temp dir (deleted on close) when absent. */
  profileDir?: string;
  /** Where failure screenshots go. Default: a temp dir. */
  screenshotDir?: string;
  /** Install the camera/mic override (spike gizzi-media.ts) before any page loads. */
  installMedia?: (ctx: BrowserContext, browser: Browser, page: Page) => Promise<void>;
  /** Forward tile state changes to the camera feed. */
  setTile?: (state: TileState) => Promise<void>;
  /** Extra Chrome flags. */
  chromeArgs?: string[];
  onEvent?: (kind: string, detail?: unknown) => void;
}

const ADMIT_POLL_MS = 1000;
const SPEAKER_POLL_MS = 250;
const CHAT_POLL_MS = 1000;
const PEOPLE_POLL_MS = 3000;
const CAPTION_POLL_MS = 500;
const ENDED_CONFIRM_CHECKS = 4; // leave button missing on 4 checks in a row (1 s apart)
const SPEAKING_CAPTION_WINDOW_MS = 1500;

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));


function hash(s: string): string {
  let h = 5381;
  for (let i = 0; i < s.length; i++) h = ((h << 5) + h + s.charCodeAt(i)) | 0;
  return (h >>> 0).toString(16);
}

/**
 * Teams refuses names with anything but letters, numbers, spaces and - ' . _ @
 * (VERIFIED 2026-11-13 on pre-join: "Your name can only include letters, numbers, spaces,
 * and these symbols"). "Gizzi (Allternit)" becomes "Gizzi - Allternit".
 */
export function teamsDisplayName(name: string): string {
  const out = name
    .replace(/\s*\(([^)]*)\)\s*/g, " - $1 ")
    .replace(/[^\p{L}\p{N} \-'._@]/gu, "")
    .replace(/\s+/g, " ")
    .replace(/ -$/, "")
    .trim();
  return (out || "Gizzi").slice(0, 100);
}

/** Roster rows read "Name, Muted, Organizer" or "Name (Guest)"; keep the name. */
function cleanName(raw: string): string {
  return raw
    .split("\n")[0]
    .replace(/\s*\((Guest|Unverified|External|Organizer|Presenter|Attendee)\)\s*$/i, "")
    .split(",")[0]
    .trim();
}

interface RawChat {
  id: string;
  sender: string;
  text: string;
}
interface RawCaption {
  speaker: string;
  text: string;
}

export function createTeamsAdapter(cfg: Pick<PodConfig, "displayName">, opts: TeamsAdapterOptions = {}): PlatformAdapter {
  let chrome: ChildProcess | null = null;
  let browser: Browser | null = null;
  let context: BrowserContext | null = null;
  let page: Page | null = null;
  let profile: string | null = null;
  let ownProfile = false;
  let displayName = teamsDisplayName(cfg.displayName);
  let live = false;
  let leftByUs = false;
  let closed = false;
  let lastFailure: AdmitResult["reason"] | undefined;

  const chatSubs = new Set<(m: ChatMessage) => void>();
  const peopleSubs = new Set<(all: Participant[]) => void>();
  const speakerSubs = new Set<(e: ActiveSpeakerEvent) => void>();
  const endedSubs = new Set<(r: Extract<EndReason, "removed" | "left_by_user" | "error">) => void>();
  const timers = new Map<string, ReturnType<typeof setInterval>>();
  const seenChat = new Set<string>();
  const ownPosts = new Set<string>();
  let lastPeopleKey = "";
  let lastPeople: Participant[] = [];
  let missingLeave = 0;
  let captionsOn = false;
  let lastCaptionSpeaker = "";
  let lastCaptionAt = 0;
  const captionSeen = new Map<string, string>();

  // ---- page helpers -------------------------------------------------------------

  async function visibleButton(m: TextMatch) {
    if (!page) return null;
    const b = page.getByRole("button", { name: rx(m) }).first();
    return (await b.isVisible().catch(() => false)) ? b : null;
  }
  async function clickIfVisible(m: TextMatch): Promise<boolean> {
    const b = await visibleButton(m);
    if (!b) return false;
    await b.click({ timeout: 3000 }).catch(() => undefined);
    return true;
  }
  async function textVisible(m: TextMatch): Promise<boolean> {
    if (!page || page.isClosed()) return false;
    return page.getByText(rx(m)).first().isVisible().catch(() => false);
  }
  async function cssVisible(css: string): Promise<boolean> {
    if (!page || page.isClosed()) return false;
    return page.locator(css).first().isVisible().catch(() => false);
  }
  async function inCall(): Promise<boolean> {
    return (await cssVisible(teams.leaveButtonCss.css)) || (await visibleButton(teams.leaveButton)) !== null;
  }
  async function shot(name: string): Promise<string | undefined> {
    if (!page || page.isClosed()) return undefined;
    const dir = opts.screenshotDir ?? join(tmpdir(), "gizzi-teams-shots");
    mkdirSync(dir, { recursive: true });
    const path = join(dir, `${name}-${Date.now()}.png`);
    return page.screenshot({ path }).then(() => path, () => undefined);
  }
  async function fail(reason: Extract<EndReason, "not_admitted" | "guest_blocked" | "join_failed" | "error">, detail: string): Promise<JoinResult> {
    lastFailure = reason;
    return { ok: false, reason, detail, screenshot: await shot(reason) };
  }

  /** True when a checkbox-style toggle exists and is checked. */
  async function toggleChecked(css: string): Promise<boolean | null> {
    if (!page) return null;
    const el = page.locator(css).first();
    if (!(await el.count().catch(() => 0))) return null;
    return el.isChecked({ timeout: 1000 }).catch(() => null);
  }
  async function setToggle(css: string, want: boolean): Promise<void> {
    if (!page) return;
    const cur = await toggleChecked(css);
    if (cur === null || cur === want) return;
    // The input is visually hidden under a styled switch; click the input via its label parent.
    await page.locator(css).first().click({ timeout: 3000, force: true }).catch(() => undefined);
  }

  // ---- launch -------------------------------------------------------------------

  async function launch(): Promise<void> {
    // The shared launcher (chrome.ts): window modes for a user's own computer
    // (MEETING_BOT_WINDOW), Chrome/Edge/Chromium probe on macOS/Windows/Linux,
    // the crash-bubble / unsupported-flag suppression and the sandbox rule.
    const session = await launchChrome({
      chromePath: opts.chromePath,
      headless: opts.headless,
      profileDir: opts.profileDir,
      chromeArgs: [...(opts.installMedia ? [] : ["--use-fake-device-for-media-stream"]), ...(opts.chromeArgs ?? [])],
      onEvent: opts.onEvent,
    });
    chrome = session.chrome;
    profile = session.profileDir;
    ownProfile = !opts.profileDir;
    browser = session.browser;
    context = session.context;
    for (const origin of ["https://teams.microsoft.com", "https://teams.live.com", "https://teams.cloud.microsoft"]) {
      await context.grantPermissions(["camera", "microphone"], { origin }).catch(() => undefined);
    }
    page = session.page;
    if (opts.installMedia) await opts.installMedia(context, browser, page);
  }

  // ---- join ---------------------------------------------------------------------

  async function prejoin(req: JoinRequest): Promise<JoinResult | null> {
    const deadline = Date.now() + req.prejoinTimeoutMs;
    let named = false;
    let nameSeen = false;
    while (Date.now() < deadline) {
      if (!page || page.isClosed()) return fail("error", "page closed during pre-join");
      if (await textVisible(teams.guestBlockedText)) return fail("guest_blocked", "tenant or organizer refuses anonymous join");
      // Launcher: force the browser, never the desktop app.
      await clickIfVisible(teams.continueOnBrowser);
      await clickIfVisible(teams.continueWithoutDevices);
      const nameInput = page.locator(teams.nameInput.css).first();
      if (await nameInput.isVisible().catch(() => false)) {
        nameSeen = true;
        if (!named) {
          await nameInput.fill(displayName, { timeout: 3000 }).catch(() => undefined);
          const val = await nameInput.inputValue().catch(() => "");
          named = val === displayName;
        }
      }
      if (nameSeen && named) {
        // Mic is ours (media override or fake device), so it stays on; camera on when the tile feeds it.
        await setToggle(teams.micToggle.css, true);
        await setToggle(teams.camToggle.css, Boolean(opts.installMedia));
        const joinBtn = page.locator(teams.joinButtonCss.css).first();
        const byRole = (await joinBtn.isVisible().catch(() => false)) ? joinBtn : await visibleButton(teams.joinButton);
        if (byRole && (await byRole.isEnabled().catch(() => false))) {
          await byRole.click({ timeout: 5000 });
          return null;
        }
      }
      await sleep(500);
    }
    return fail("join_failed", `pre-join did not finish in ${Math.round(req.prejoinTimeoutMs / 1000)} s`);
  }

  async function joinCall(req: JoinRequest, onPhase?: (p: JoinPhase) => void): Promise<JoinResult> {
    displayName = teamsDisplayName(req.displayName);
    try {
      if (closed) return await fail("error", "adapter closed");
      await launch();
      if (!page) return await fail("error", "no page");
      onPhase?.("prejoin");
      opts.onEvent?.("teams_goto", req.url);
      await page.goto(req.url, { waitUntil: "domcontentloaded", timeout: 60_000 });
      const pre = await prejoin(req);
      if (pre) return pre;
      onPhase?.("join_clicked");
      onPhase?.("waiting_admit");
      const admit = await waitForAdmit(req.admitTimeoutMs);
      if (!admit.admitted) return await fail(admit.reason ?? "not_admitted", "lobby ended without admission");
      onPhase?.("live");
      live = true;
      return { ok: true, liveAt: Date.now() };
    } catch (e) {
      return await fail("error", String(e).slice(0, 400));
    }
  }

  async function waitForAdmit(timeoutMs: number): Promise<AdmitResult> {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      if (!page || page.isClosed()) return { admitted: false, reason: "error" };
      // The leave button also exists in the lobby on some builds: live means the lobby copy is gone too.
      if ((await inCall()) && !(await textVisible(teams.waitingText))) {
        live = true;
        return { admitted: true };
      }
      if (await textVisible(teams.notFoundText)) return { admitted: false, reason: "join_failed" };
      if (await textVisible(teams.deniedText)) return { admitted: false, reason: "not_admitted" };
      if (await textVisible(teams.guestBlockedText)) return { admitted: false, reason: "guest_blocked" };
      await sleep(ADMIT_POLL_MS);
    }
    return { admitted: false, reason: "not_admitted" };
  }

  // ---- chat ---------------------------------------------------------------------

  async function openChat(): Promise<boolean> {
    if (!page) return false;
    const input = page.locator(teams.chatInput.css).first();
    if (await input.isVisible().catch(() => false)) return true;
    if (!(await clickIfVisible(teams.chatButton))) {
      const css = page.locator(teams.chatButtonCss.css).first();
      if (await css.isVisible().catch(() => false)) await css.click({ timeout: 3000 }).catch(() => undefined);
    }
    return input.waitFor({ state: "visible", timeout: 5000 }).then(
      () => true,
      () => false,
    );
  }

  async function postChat(text: string): Promise<boolean> {
    if (!page || page.isClosed() || !live) return false;
    try {
      if (!(await openChat())) return false; // no chat for anonymous users in this tenant
      if (await textVisible(teams.chatDisabledText)) return false;
      const input = page.locator(teams.chatInput.css).first();
      await input.click({ timeout: 3000 });
      await input.fill(text, { timeout: 3000 });
      ownPosts.add(text.trim());
      const send = page.locator(teams.chatSend.css).first();
      if (await send.isVisible().catch(() => false)) await send.click({ timeout: 3000 });
      else await input.press("Enter");
      return true;
    } catch {
      return false;
    }
  }

  async function readChat(): Promise<RawChat[]> {
    if (!page || page.isClosed()) return [];
    return page.evaluate(
      ({ msgCss, idCss, authorCss, bodyCss }) =>
        Array.from(document.querySelectorAll<HTMLElement>(msgCss)).map((el) => {
          const idEl = el.matches(idCss) ? el : el.querySelector<HTMLElement>(idCss);
          const author = el.querySelector<HTMLElement>(authorCss);
          const body = el.querySelector<HTMLElement>(bodyCss);
          return {
            id: idEl?.getAttribute("data-mid") ?? "",
            sender: (author?.innerText ?? "").trim(),
            text: (body?.innerText ?? el.innerText ?? "").trim(),
          };
        }),
      { msgCss: teams.chatMessage.css, idCss: teams.chatMessageId.css, authorCss: teams.chatAuthor.css, bodyCss: teams.chatBody.css },
    ).catch(() => [] as RawChat[]);
  }

  async function pollChat(): Promise<void> {
    let lastSender = "";
    for (const m of await readChat()) {
      if (!m.text) continue;
      // Consecutive messages from one sender show the author once.
      const sender = m.sender || lastSender;
      lastSender = sender;
      const id = m.id || hash(`${sender}:${m.text}`);
      if (seenChat.has(id)) continue;
      seenChat.add(id);
      const self = ownPosts.has(m.text) || sender === "You" || sender === displayName;
      if (self) continue;
      const msg: ChatMessage = { id, sender, text: m.text, t: Date.now(), self: false };
      for (const cb of chatSubs) cb(msg);
    }
  }

  // ---- participants -------------------------------------------------------------

  async function readPeople(): Promise<Participant[]> {
    if (!page || page.isClosed()) return [];
    const rows = await page.evaluate(
      ({ rowCss, nameCss }) =>
        Array.from(document.querySelectorAll<HTMLElement>(rowCss)).map((el) => {
          const n = el.querySelector<HTMLElement>(nameCss);
          return { name: (n?.innerText ?? el.getAttribute("aria-label") ?? el.innerText ?? "").trim(), raw: el.innerText ?? "" };
        }),
      { rowCss: teams.participantRows.css, nameCss: teams.participantRowName.css },
    ).catch(() => [] as { name: string; raw: string }[]);
    const out: Participant[] = [];
    const seen = new Set<string>();
    for (const r of rows) {
      const name = cleanName(r.name);
      if (!name || seen.has(name)) continue;
      if (name === displayName || /\(You\)$/i.test(r.name) || /^you$/i.test(name)) continue;
      seen.add(name);
      const p: Participant = { id: name, name };
      if (/\((Guest|Unverified|External)\)/i.test(r.raw)) p.internal = false;
      out.push(p);
    }
    return out;
  }

  async function ensurePeoplePanel(): Promise<void> {
    if (!page || (await cssVisible(teams.participantRows.css))) return;
    if (!(await clickIfVisible(teams.peopleButton))) {
      const css = page.locator(teams.peopleButtonCss.css).first();
      if (await css.isVisible().catch(() => false)) await css.click({ timeout: 3000 }).catch(() => undefined);
    }
    await sleep(500);
  }

  async function participants(): Promise<Participant[]> {
    if (!live) return [];
    await ensurePeoplePanel();
    const all = await readPeople();
    if (all.length || !lastPeople.length) lastPeople = all;
    return all.length ? all : lastPeople;
  }

  async function pollPeople(): Promise<void> {
    const all = await participants();
    const key = all.map((p) => p.id).sort().join("\u0000");
    if (key === lastPeopleKey) return;
    lastPeopleKey = key;
    for (const cb of peopleSubs) cb(all);
  }

  // ---- active speaker -----------------------------------------------------------

  async function pollSpeaker(): Promise<void> {
    if (!page || page.isClosed()) return;
    const names = await page.evaluate(
      ({ tileCss, speakCss }) => {
        const out: string[] = [];
        for (const tile of Array.from(document.querySelectorAll<HTMLElement>(tileCss))) {
          if (!tile.querySelector(speakCss)) continue;
          const label = (tile.getAttribute("aria-label") ?? tile.innerText ?? "").trim();
          if (label) out.push(label);
        }
        return out;
      },
      { tileCss: teams.stageTile.css, speakCss: teams.speakingIndicator.css },
    ).catch(() => [] as string[]);
    const now = Date.now();
    const set = new Set(names.map(cleanName).filter((n) => n && n !== displayName && !/\(You\)$/i.test(n)));
    // Captions name the speaker when the DOM indicator is not found (Teams has no ARIA for it).
    if (!set.size && captionsOn && lastCaptionSpeaker && now - lastCaptionAt <= SPEAKING_CAPTION_WINDOW_MS) set.add(lastCaptionSpeaker);
    const e: ActiveSpeakerEvent = { t: now, names: [...set] };
    for (const cb of speakerSubs) cb(e);
  }

  // ---- captions -----------------------------------------------------------------

  async function captions(cb: (e: CaptionEvent) => void): Promise<boolean> {
    if (!page || page.isClosed() || !live) return false;
    try {
      if (!(await cssVisible(teams.captionsWindow.css))) {
        await page.keyboard.press("Control+Shift+U").catch(() => undefined); // Teams shortcut for live captions
        await sleep(1500);
      }
      if (!(await cssVisible(teams.captionsWindow.css))) {
        await clickIfVisible(teams.moreButton);
        await sleep(400);
        await page.getByText(rx(teams.languageSpeechMenuItem)).first().click({ timeout: 2000 }).catch(() => undefined);
        await sleep(400);
        await page.getByText(rx(teams.captionsMenuItem)).first().click({ timeout: 2000 }).catch(() => undefined);
        await sleep(1500);
      }
      if (!(await cssVisible(teams.captionsWindow.css))) return false;
    } catch {
      return false;
    }
    captionsOn = true;
    startTimer("captions", CAPTION_POLL_MS, async () => {
      if (!page || page.isClosed()) return;
      const rows = await page.evaluate(
        ({ rowCss, authorCss, textCss }) =>
          Array.from(document.querySelectorAll<HTMLElement>(rowCss)).map((el) => ({
            speaker: (el.querySelector<HTMLElement>(authorCss)?.innerText ?? "").trim(),
            text: (el.querySelector<HTMLElement>(textCss)?.innerText ?? "").trim(),
          })),
        { rowCss: teams.captionRow.css, authorCss: teams.captionAuthor.css, textCss: teams.captionText.css },
      ).catch(() => [] as RawCaption[]);
      const now = Date.now();
      let n = 0;
      for (const r of rows) {
        if (!r.text) continue;
        const key = `${n++}:${r.speaker}`;
        const prev = captionSeen.get(key);
        if (prev === r.text) continue;
        captionSeen.set(key, r.text);
        lastCaptionSpeaker = r.speaker || lastCaptionSpeaker;
        lastCaptionAt = now;
        // A line is final once the next row exists, i.e. it is not the last row.
        cb({ t: now, speaker: r.speaker, text: r.text, final: n < rows.length });
      }
      // Rows scroll out of the pane; forget keys beyond the current length.
      for (const k of [...captionSeen.keys()]) if (Number(k.split(":")[0]) >= rows.length) captionSeen.delete(k);
    });
    return true;
  }

  // ---- timers, end detection ----------------------------------------------------

  function startTimer(name: string, ms: number, fn: () => Promise<void>): void {
    if (timers.has(name)) return;
    let busy = false;
    timers.set(
      name,
      setInterval(() => {
        if (busy || closed) return;
        busy = true;
        fn().catch(() => undefined).finally(() => (busy = false));
      }, ms),
    );
  }
  function stopTimer(name: string): void {
    const t = timers.get(name);
    if (t) clearInterval(t);
    timers.delete(name);
  }

  function emitEnded(reason: Extract<EndReason, "removed" | "left_by_user" | "error">): void {
    if (!live) return;
    live = false;
    for (const cb of endedSubs) cb(reason);
  }

  function startEndWatch(): void {
    startTimer("ended", 1000, async () => {
      if (!live) return;
      if (!page || page.isClosed()) return emitEnded(leftByUs ? "left_by_user" : "error");
      if (await inCall()) {
        missingLeave = 0;
        return;
      }
      if (++missingLeave >= ENDED_CONFIRM_CHECKS) {
        emitEnded(leftByUs ? "left_by_user" : "removed");
      }
    });
  }

  // ---- subscriptions ------------------------------------------------------------

  function sub<T>(set: Set<T>, cb: T, timer: string, ms: number, fn: () => Promise<void>): Unsubscribe {
    set.add(cb);
    startTimer(timer, ms, fn);
    startEndWatch();
    return () => {
      set.delete(cb);
      if (!set.size) stopTimer(timer);
    };
  }

  async function leave(): Promise<void> {
    if (leftByUs && !live) return;
    leftByUs = true;
    if (page && !page.isClosed()) {
      const wasLive = live;
      live = false;
      const btn = page.locator(teams.leaveButtonCss.css).first();
      if (await btn.isVisible().catch(() => false)) await btn.click({ timeout: 3000 }).catch(() => undefined);
      else await clickIfVisible(teams.leaveButton);
      await sleep(500);
      await clickIfVisible(teams.leaveConfirm); // confirmation in some tenants
      await sleep(1000);
      if (wasLive) for (const cb of endedSubs) cb("left_by_user");
    }
  }

  async function close(): Promise<void> {
    if (closed) return;
    closed = true;
    live = false;
    for (const name of [...timers.keys()]) stopTimer(name);
    await browser?.close().catch(() => undefined);
    chrome?.kill("SIGTERM");
    await sleep(300);
    if (profile && ownProfile) rmSync(profile, { recursive: true, force: true });
  }

  void lastFailure;

  return {
    platform: "teams",
    join: joinCall,
    waitForAdmit,
    postChat,
    onChat: (cb) => sub(chatSubs, cb, "chat", CHAT_POLL_MS, pollChat),
    participants,
    onParticipants: (cb) => sub(peopleSubs, cb, "people", PEOPLE_POLL_MS, pollPeople),
    onActiveSpeaker: (cb) => sub(speakerSubs, cb, "speaker", SPEAKER_POLL_MS, pollSpeaker),
    captions,
    setTile: async (state) => {
      await opts.setTile?.(state);
    },
    leave,
    onEnded: (cb) => {
      endedSubs.add(cb);
      startEndWatch();
      return () => void endedSubs.delete(cb);
    },
    close,
  };
}

/** Factory in the shape the runner expects; pass options through a wrapper when the pod has media hooks. */
export const teamsFactory: AdapterFactory = (cfg) => createTeamsAdapter(cfg);
