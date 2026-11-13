// Allternit Meetings lane 2.2: Google Meet adapter (implements adapter.ts).
//
// Identity: a signed-in bot Google account (CONTRACT section 1; free-Gmail calls
// refuse guests and automation-mode browsers). The profile directory is the
// bot account's persisted Chrome profile (docs/meetings/BOT-ACCOUNTS.md); this
// adapter never signs in. If the profile turns out signed out, it still tries
// as a guest and reports `bot_profile_signed_out` so the account gets a human
// re-sign-in.
//
// Flow (from spike/join.ts): open the link -> pre-join (dismiss nudges, mic +
// camera on, name only when signed out) -> "Join now" / "Ask to join" ->
// waitForAdmit (decline or "You can't join" is final) -> live. Then polls:
// tiles every 250 ms (participants + active speaker), chat every 1 s, the
// Leave button every 1 s (gone 4 times in a row = removed or call ended).
// Captions are an opt-in cross-check on speaker labels.
//
// Media: the runner's `installMedia` hook (media.ts PodMedia.install) puts the
// camera/mic override and the call capture in place before Meet loads, and
// `setTile` drives the camera tile. Every selector lives in meet-selectors.ts.

import { mkdirSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Browser, BrowserContext, Page } from "playwright-core";
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
import { launchChrome, type ChromeSession } from "../chrome.js";
import { meet, rx, type TextMatch } from "./meet-selectors.js";

export interface MeetAdapterOptions {
  chromePath?: string;
  headless?: boolean;
  /** Overrides the identity's profile dir. Absent and no google_account identity = a temp profile. */
  profileDir?: string;
  screenshotDir?: string;
  /** Camera/mic override + call capture, installed before the meeting page loads (media.ts). */
  installMedia?: (ctx: BrowserContext, browser: Browser, page: Page) => Promise<void>;
  setTile?: (state: TileState) => Promise<void>;
  chromeArgs?: string[];
  onEvent?: (kind: string, detail?: unknown) => void;
}

const SPEAKER_POLL_MS = 250;
const CHAT_POLL_MS = 1000;
const LEAVE_CHECK_MS = 1000;
const DIALOG_CHECK_MS = 5000;
const CAPTION_POLL_MS = 500;
const CAPTION_FINAL_MS = 1500;
const ENDED_CONFIRM_CHECKS = 4;

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

function hash(s: string): string {
  let h = 5381;
  for (let i = 0; i < s.length; i++) h = ((h << 5) + h + s.charCodeAt(i)) | 0;
  return (h >>> 0).toString(16);
}

interface Snapshot {
  others: Array<{ id: string; name: string; speaking: boolean }>;
  selfSpeaking: boolean;
  peopleCount: number | null;
}

interface RawChat {
  id: string;
  sender: string;
  text: string;
}

export function createMeetAdapter(cfg: Pick<PodConfig, "displayName" | "identity">, opts: MeetAdapterOptions = {}): PlatformAdapter {
  const log = opts.onEvent ?? (() => undefined);
  let session: ChromeSession | null = null;
  let page: Page | null = null;
  let displayName = cfg.displayName;
  let onPhase: ((p: JoinPhase) => void) | undefined;
  let phase: JoinPhase | null = null;
  let live = false;
  let leftByUs = false;
  let closed = false;
  let endedFired = false;
  let signedOutReported = false;

  const chatSubs = new Set<(m: ChatMessage) => void>();
  const peopleSubs = new Set<(all: Participant[]) => void>();
  const speakerSubs = new Set<(e: ActiveSpeakerEvent) => void>();
  const endedSubs = new Set<(r: Extract<EndReason, "removed" | "left_by_user" | "error">) => void>();
  const timers = new Set<ReturnType<typeof setInterval>>();
  const seenChat = new Set<string>();
  const ownPosts = new Set<string>();
  let people: Participant[] = [];
  let peopleKey = "";
  let missingLeave = 0;

  const screenshotDir = opts.screenshotDir ?? mkdtempSync(join(tmpdir(), "gizzi-meet-shots-"));
  mkdirSync(screenshotDir, { recursive: true });

  function setPhase(p: JoinPhase): void {
    if (phase === p) return;
    phase = p;
    log("meet_phase", p);
    onPhase?.(p);
  }

  // ---- page helpers ------------------------------------------------------------
  async function visibleButton(m: TextMatch) {
    if (!page || page.isClosed()) return null;
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
  async function shot(name: string): Promise<string | undefined> {
    if (!page || page.isClosed()) return undefined;
    const path = join(screenshotDir, `meet-${name}-${Date.now()}.png`);
    return page.screenshot({ path }).then(() => path, () => undefined);
  }

  // ---- in-call reads (run in the page) ----------------------------------------------
  async function snapshot(): Promise<Snapshot | null> {
    if (!page || page.isClosed()) return null;
    return page
      .evaluate(
        ({ tileCss, nameCss, speakCss, peopleCss, self }) => {
          const byId = new Map<string, { id: string; name: string; speaking: boolean; isSelf: boolean }>();
          for (const tile of Array.from(document.querySelectorAll<HTMLElement>(tileCss))) {
            const id = tile.getAttribute("data-participant-id") ?? "";
            const isSelf = tile.querySelector("[data-self-name]") !== null;
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
            // Camera + screen-share tiles of one person share the id: merge.
            byId.set(id, {
              id,
              name: prev?.name || name || id,
              speaking: Boolean(prev?.speaking) || speaking,
              isSelf: Boolean(prev?.isSelf) || isSelf || name === self || name === "You",
            });
          }
          let peopleCount: number | null = null;
          const btn = document.querySelector<HTMLElement>(peopleCss);
          if (btn) {
            const m = /(\d+)/.exec(`${btn.getAttribute("aria-label") ?? ""} ${btn.textContent ?? ""}`);
            if (m) peopleCount = Number(m[1]);
          }
          const all = Array.from(byId.values());
          return {
            others: all.filter((p) => !p.isSelf && p.id).map(({ id, name, speaking }) => ({ id, name, speaking })),
            selfSpeaking: all.some((p) => p.isSelf && p.speaking),
            peopleCount,
          };
        },
        { tileCss: meet.participantTile.css, nameCss: meet.tileName.css, speakCss: meet.speakingClasses.css, peopleCss: meet.peopleCount.css, self: displayName },
      )
      .catch(() => null);
  }

  async function readChat(): Promise<RawChat[]> {
    if (!page || page.isClosed()) return [];
    return page
      .evaluate(
        ({ msgCss, senderCss }) =>
          Array.from(document.querySelectorAll<HTMLElement>(msgCss)).map((el) => {
            const group = el.closest<HTMLElement>(senderCss);
            const sender =
              group?.getAttribute("data-sender-name") ?? (group?.firstElementChild as HTMLElement | null)?.innerText.split("\n")[0] ?? "";
            return { id: el.getAttribute("data-message-id") ?? "", sender: sender.trim(), text: el.innerText.trim() };
          }),
        { msgCss: meet.chatMessage.css, senderCss: meet.chatSenderGroup.css },
      )
      .catch(() => []);
  }

  async function openChat(): Promise<boolean> {
    if (!page || page.isClosed()) return false;
    const input = page.locator(meet.chatInput.css).first();
    if (await input.isVisible().catch(() => false)) return true;
    await clickIfVisible(meet.chatButton);
    return input.waitFor({ state: "visible", timeout: 5000 }).then(
      () => true,
      () => false,
    );
  }

  function every(ms: number, fn: () => Promise<void>): void {
    let busy = false;
    const t = setInterval(() => {
      if (busy || closed) return;
      busy = true;
      void fn().finally(() => (busy = false));
    }, ms);
    timers.add(t);
  }

  function fireEnded(reason: Extract<EndReason, "removed" | "left_by_user" | "error">): void {
    if (endedFired) return;
    endedFired = true;
    live = false;
    log("meet_ended", reason);
    for (const cb of endedSubs) cb(reason);
  }

  function startPolls(): void {
    every(SPEAKER_POLL_MS, async () => {
      const snap = await snapshot();
      if (!snap) return;
      const t = Date.now();
      const e: ActiveSpeakerEvent = { t, names: snap.others.filter((p) => p.speaking).map((p) => p.name) };
      for (const cb of speakerSubs) cb(e);
      // Tiles undercount in big calls and while someone presents; when the
      // toolbar count says others are here, never report an empty room from tiles.
      if (snap.others.length === 0 && snap.peopleCount !== null && snap.peopleCount > 1 && people.length > 0) return;
      const next = snap.others.map((p) => ({ id: p.id, name: p.name }));
      const key = next.map((p) => `${p.id}\u0000${p.name}`).sort().join("\u0001");
      if (key !== peopleKey) {
        peopleKey = key;
        people = next;
        for (const cb of peopleSubs) cb(people);
      }
    });

    every(CHAT_POLL_MS, async () => {
      if (!chatSubs.size) return;
      for (const m of await readChat()) {
        const id = m.id || `h${hash(`${m.sender}\u0000${m.text}`)}`;
        if (seenChat.has(id)) continue;
        seenChat.add(id);
        const self = m.sender === "You" || m.sender === displayName || ownPosts.has(m.text);
        if (self) continue;
        const msg: ChatMessage = { id, sender: m.sender, text: m.text, t: Date.now(), self };
        for (const cb of chatSubs) cb(msg);
      }
    });

    every(LEAVE_CHECK_MS, async () => {
      if (!live) return;
      if (await visibleButton(meet.leaveButton)) {
        missingLeave = 0;
        return;
      }
      if (++missingLeave >= ENDED_CONFIRM_CHECKS) {
        if (leftByUs) return fireEnded("left_by_user");
        const removed = (await textVisible(meet.removedText)) || (await textVisible(meet.deniedText));
        log("meet_leave_button_gone", { removedText: removed });
        await shot("ended");
        fireEnded("removed");
      }
    });

    every(DIALOG_CHECK_MS, async () => {
      if (live && (await clickIfVisible(meet.inCallDismiss))) log("meet_dialog_dismissed");
    });
  }

  // ---- join ----------------------------------------------------------------------
  async function prejoin(req: JoinRequest): Promise<JoinResult | null> {
    setPhase("prejoin");
    const deadline = Date.now() + req.prejoinTimeoutMs;
    let named = false;
    while (Date.now() < deadline) {
      if (await textVisible(meet.guestBlockedText)) {
        return { ok: false, reason: "guest_blocked", detail: "meet refused this account", screenshot: await shot("guest-blocked") };
      }
      if (await textVisible(meet.deniedText)) {
        return { ok: false, reason: "not_admitted", detail: "meet refused the join", screenshot: await shot("denied") };
      }
      await clickIfVisible(meet.continueWithoutDevices);
      const nameInput = page!.locator(meet.nameInput.css).first();
      const nameVisible = await nameInput.isVisible().catch(() => false);
      if (nameVisible && req.identity.kind === "google_account" && !signedOutReported) {
        // The guest name field only shows when the profile is signed out.
        signedOutReported = true;
        log("bot_profile_signed_out", { account: req.identity.accountEmail });
      }
      if (nameVisible) await clickIfVisible(meet.dismissSignIn);
      if (!named && nameVisible) {
        await nameInput.fill(req.displayName);
        named = true;
        log("meet_name_filled", req.displayName);
      }
      if (await clickIfVisible(meet.micOn)) log("meet_mic_on");
      if (opts.installMedia && (await clickIfVisible(meet.camOn))) log("meet_cam_on");
      const joinBtn = await visibleButton(meet.joinButton);
      if (joinBtn && (named || !nameVisible)) {
        const label = ((await joinBtn.textContent().catch(() => "")) ?? "").trim();
        await joinBtn.click({ timeout: 5000 });
        log("meet_join_clicked", label);
        setPhase("join_clicked");
        return null;
      }
      await sleep(500);
    }
    return { ok: false, reason: "join_failed", detail: "pre-join screen did not offer a join button in time", screenshot: await shot("prejoin-timeout") };
  }

  async function waitForAdmit(timeoutMs: number): Promise<AdmitResult> {
    if (!page) return { admitted: false, reason: "error" };
    const start = Date.now();
    const deadline = start + timeoutMs;
    while (Date.now() < deadline && !closed) {
      const waiting = await textVisible(meet.waitingText);
      // "Leave call" shows in some waiting rooms too; live means the waiting copy is gone.
      if (!waiting && (await visibleButton(meet.leaveButton))) return { admitted: true };
      if (waiting || Date.now() - start > 3000) setPhase("waiting_admit");
      if (await textVisible(meet.deniedText)) {
        await shot("denied");
        return { admitted: false, reason: "not_admitted" };
      }
      if (await textVisible(meet.guestBlockedText)) {
        await shot("guest-blocked");
        return { admitted: false, reason: "guest_blocked" };
      }
      await sleep(1000);
    }
    await shot("admit-timeout");
    return { admitted: false, reason: "not_admitted" };
  }

  // ---- the adapter -------------------------------------------------------------------
  return {
    platform: "meet",

    async join(req, cb) {
      onPhase = cb;
      displayName = req.displayName;
      try {
        const profileDir = opts.profileDir ?? (req.identity.kind === "google_account" ? req.identity.profileDir : undefined);
        session = await launchChrome({ chromePath: opts.chromePath, headless: opts.headless, profileDir, chromeArgs: opts.chromeArgs, onEvent: log });
        page = session.page;
        page.on("pageerror", (e) => log("meet_pageerror", String(e).slice(0, 300)));
        await session.context.grantPermissions(["camera", "microphone"], { origin: "https://meet.google.com" }).catch(() => undefined);
        if (opts.installMedia) await opts.installMedia(session.context, session.browser, page);
        await page.goto(req.url, { waitUntil: "domcontentloaded", timeout: 60_000 });

        const failed = await prejoin(req);
        if (failed) return failed;
        const admit = await waitForAdmit(req.admitTimeoutMs);
        if (!admit.admitted) {
          return { ok: false, reason: admit.reason ?? "not_admitted", detail: signedOutReported ? "bot_profile_signed_out" : undefined };
        }
        live = true;
        const liveAt = Date.now();
        setPhase("live");
        startPolls();
        return { ok: true, liveAt, detail: signedOutReported ? "bot_profile_signed_out" : undefined };
      } catch (e) {
        return { ok: false, reason: "error", detail: String(e).slice(0, 300), screenshot: await shot("error") };
      }
    },

    waitForAdmit,

    async postChat(text) {
      if (!page || page.isClosed()) return false;
      if (!(await openChat())) return false;
      const input = page.locator(meet.chatInput.css).first();
      ownPosts.add(text.trim());
      await input.fill(text).catch(() => undefined);
      if (!(await clickIfVisible(meet.chatSend))) await input.press("Enter").catch(() => undefined);
      return true;
    },

    onChat(cb) {
      chatSubs.add(cb);
      // The chat panel stays open so messages render (it shares the side panel with People).
      void openChat();
      return () => chatSubs.delete(cb);
    },

    async participants() {
      return people;
    },

    onParticipants(cb) {
      peopleSubs.add(cb);
      return () => peopleSubs.delete(cb);
    },

    onActiveSpeaker(cb) {
      speakerSubs.add(cb);
      return () => speakerSubs.delete(cb);
    },

    async captions(cb) {
      if (!page || page.isClosed()) return false;
      if (!(await clickIfVisible(meet.captionsOn))) {
        if (!(await page.locator(meet.captionsRegion.css).first().isVisible().catch(() => false))) return false;
      }
      const ok = await page
        .locator(meet.captionsRegion.css)
        .first()
        .waitFor({ state: "visible", timeout: 5000 })
        .then(() => true, () => false);
      if (!ok) return false;
      // Per block: emit while the text grows, then once more as final after it settles.
      const last = new Map<string, { speaker: string; text: string; changed: number; final: boolean }>();
      every(CAPTION_POLL_MS, async () => {
        if (!page || page.isClosed()) return;
        const blocks = await page
          .evaluate(
            ({ regionCss, speakerCss, textCss }) => {
              const region = document.querySelector<HTMLElement>(regionCss);
              if (!region) return [] as Array<{ speaker: string; text: string }>;
              return Array.from(region.children).map((b) => {
                const sp = b.querySelector<HTMLElement>(speakerCss);
                const speaker = (sp?.getAttribute("alt") || sp?.textContent || "").trim();
                const text = (b.querySelector<HTMLElement>(textCss)?.innerText ?? (b as HTMLElement).innerText ?? "").trim();
                return { speaker, text };
              });
            },
            { regionCss: meet.captionsRegion.css, speakerCss: meet.captionSpeaker.css, textCss: meet.captionText.css },
          )
          .catch(() => [] as Array<{ speaker: string; text: string }>);
        const now = Date.now();
        blocks.forEach((b, i) => {
          if (!b.text) return;
          const key = `${i}:${b.speaker}`;
          const prev = last.get(key);
          if (!prev || prev.text !== b.text) {
            last.set(key, { speaker: b.speaker, text: b.text, changed: now, final: false });
            cb({ t: now, speaker: b.speaker, text: b.text, final: false } satisfies CaptionEvent);
          }
        });
        for (const v of last.values()) {
          if (!v.final && now - v.changed >= CAPTION_FINAL_MS) {
            v.final = true;
            cb({ t: v.changed, speaker: v.speaker, text: v.text, final: true });
          }
        }
        if (last.size > 200) last.clear();
      });
      return true;
    },

    async setTile(state) {
      await opts.setTile?.(state);
    },

    async leave() {
      if (leftByUs) return;
      leftByUs = true;
      if (page && !page.isClosed() && (await clickIfVisible(meet.leaveButton))) {
        log("meet_left");
        await sleep(1000);
      }
      fireEnded("left_by_user");
    },

    onEnded(cb) {
      endedSubs.add(cb);
      return () => endedSubs.delete(cb);
    },

    async close() {
      if (closed) return;
      closed = true;
      for (const t of timers) clearInterval(t);
      timers.clear();
      await session?.close();
      session = null;
      page = null;
    },
  };
}

export const meetFactory: AdapterFactory = (cfg) => createMeetAdapter(cfg);
