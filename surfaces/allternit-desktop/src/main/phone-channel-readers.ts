/**
 * Channel readers: read and send messages in a messaging app on the owner's
 * linked phone by driving its real UI (Eoj, 2026-10-18: "lanes that need an
 * adapter can still be read — do the adapter"). Each app has a spec that knows
 * its screens; the reader runs one operation at a time (it drives the phone),
 * keeps a local copy of what it read under the Desktop's user data, and never
 * uploads it.
 *
 * Tools used: phone.open_app, phone.read_ui, phone.find_and_tap, phone.tap,
 * phone.type, phone.back — the same runner bots use, so approvals, rate
 * limits and the activity indicator apply.
 *
 * v1 reads what is on screen (the conversation list and the visible part of a
 * conversation). Scrolling back for older messages is a later step.
 *
 * Text is read as text. Anything else in a message (images, link cards,
 * embeds, stickers, attachments) is cut out of one screenshot taken with the
 * read, at its place on screen, so the bot phone shows what it really looks
 * like rather than its accessibility description (Eoj, 2026-10-18).
 */
import * as fs from 'node:fs';
import * as path from 'node:path';
import { centerOf, nodesInside, parseUiTree, type UiNode } from './phone-ui-tree.js';

export interface ChannelConversation {
  /** Stable within the app: "dm:<name>" or "<server>/<channel>". */
  id: string;
  title: string;
  /** Where it lives: "Direct messages" or the server/group name. */
  group: string;
  preview: string | null;
  time: string | null;
  unread: number;
}

type Rect = { x: number; y: number; w: number; h: number };

/** A piece of a message: text, or a picture of a non-text element. `alt` is its text for search and screen readers. */
export type MessagePart =
  | { kind: 'text'; text: string }
  | { kind: 'image'; alt: string; src?: string; width: number; height: number; crop?: Rect;
      /** Where it sat on the phone's screen (for playing its clip). */
      at?: Rect;
      /** It moved between two screenshots: a GIF, animated sticker or video. */
      animated?: boolean };

/** A few seconds of the phone's screen, recorded on request to play animated parts. */
export interface ChannelClip {
  /** data:video/mp4 URL of the whole screen. */
  src: string;
  /** The screen size the parts' `at` rects use. */
  screenWidth: number;
  screenHeight: number;
  recordedAt: string;
}

export interface ChannelMessage {
  author: string;
  time: string | null;
  /** Plain-text version (previews, search, older copies). */
  text: string;
  mine: boolean;
  parts?: MessagePart[];
}

export type ReaderResult<T> = { ok: true; data: T; readAt: string } | { ok: false; error: string; message: string };

export type PhoneCall = (name: string, args: Record<string, unknown>) => Promise<{ ok: boolean; error?: string; message?: string; text?: string } & Record<string, unknown>>;

/** What a spec drives: the tool runner bound to one phone. */
export interface ReaderNav {
  call: PhoneCall;
  read(): Promise<UiNode[]>;
  tapNode(n: UiNode): Promise<void>;
  tapLabel(label: string): Promise<boolean>;
  wait(ms: number): Promise<void>;
}

export interface ChannelAppSpec {
  /** Phone app catalog id. */
  app: string;
  name: string;
  conversations(nav: ReaderNav): Promise<ChannelConversation[]>;
  /** Leaves the app showing that conversation. */
  open(nav: ReaderNav, convo: ChannelConversation): Promise<void>;
  messages(nodes: UiNode[], me: string | null): ChannelMessage[];
  /** Types into the open conversation's composer and sends. */
  send(nav: ReaderNav, text: string): Promise<void>;
  /** The signed-in account name, read from a screen the spec visits. */
  me?(nodes: UiNode[]): string | null;
}

class ReaderError extends Error {
  constructor(readonly code: string, message: string) { super(message); }
}

// ── Discord ──────────────────────────────────────────────────────────────────

const DISCORD_TIME = /^(\d+[smhdwy]|\d{1,2}:\d{2}\s?[AP]M|yesterday|\d{1,2}\/\d{1,2}(\/\d{2,4})?)$/i;
// Today's messages carry only the time ('allternit, 7:26 AM'); older ones the date too.
const DISCORD_HEADER = /^(.+), ((?:\d{2}\/\d{2}\/\d{4} )?\d{1,2}:\d{2}(?: [AP]M)?|Today at .+|Yesterday at .+)$/;
const RAIL_WIDTH = 130; // the server rail on the left: DM avatars and servers

function mentionCount(label: string): number {
  const m = /^(\d+) mentions?, /.exec(label);
  return m ? Number(m[1]) : 0;
}

/** Back out of whatever Discord shows until the server rail (with "Direct Messages") is visible. */
async function discordHome(nav: ReaderNav): Promise<UiNode[]> {
  await nav.call('phone.open_app', { app: 'discord' });
  await nav.wait(1500);
  for (let i = 0; i < 4; i += 1) {
    const nodes = await nav.read();
    if (nodes.some((n) => n.label === 'Direct Messages' && n.clickable)) return nodes;
    const back = nodes.find((n) => n.clickable && n.rect && n.rect.y < 200 && /^Back\b/.test(n.label ?? ''));
    if (back) await nav.tapNode(back);
    else await nav.call('phone.back', {});
    await nav.wait(1000);
  }
  throw new ReaderError('not_signed_in', 'Discord isn’t showing its home screen. Open Discord on the phone and sign in.');
}

/**
 * Select a rail item (Direct Messages or a server) and return the screen with
 * its list showing. Tapping an item that is already selected closes the
 * drawer, so that case goes back home instead of tapping.
 */
async function discordRailItem(nav: ReaderNav, nodes: UiNode[], match: (n: UiNode) => boolean): Promise<UiNode[]> {
  const item = nodes.find((n) => n.clickable && match(n));
  if (!item) return nodes;
  if (!item.selected) {
    await nav.tapNode(item);
    await nav.wait(1200);
  }
  const after = await nav.read();
  // A selected item can still land in a conversation: back out to the list.
  return after.some((n) => n.label === 'Direct Messages') ? after : discordHome(nav);
}

const discord: ChannelAppSpec = {
  app: 'discord',
  name: 'Discord',

  me(nodes) {
    const n = nodes.find((x) => x.clickable && /, (Online|Idle|Do Not Disturb|Invisible)$/.test(x.label ?? '') && (x.rect?.y ?? 0) > 1200);
    return n?.label ? n.label.replace(/, (Online|Idle|Do Not Disturb|Invisible)$/, '') : null;
  },

  async conversations(nav) {
    let nodes = await discordHome(nav);
    const out: ChannelConversation[] = [];

    // Direct messages: the list next to the rail. Tapping an already-selected
    // rail item jumps back into the last conversation, so only tap when needed.
    nodes = await discordRailItem(nav, nodes, (n) => n.label === 'Direct Messages');
    for (const row of nodes.filter((n) => n.clickable && /\(direct message\)$/.test(n.label ?? '') && (n.rect?.x ?? 0) > RAIL_WIDTH)) {
      const texts = nodesInside(nodes, row).filter((t) => t.type === 'TextView' && t.label).map((t) => t.label as string);
      const title = texts[0] ?? row.label!.replace(/^\d+ mentions?, /, '').replace(/ \(direct message\)$/, '');
      out.push({
        id: `dm:${title}`,
        title,
        group: 'Direct messages',
        time: texts.find((t) => DISCORD_TIME.test(t)) ?? null,
        preview: texts.slice(1).find((t) => !DISCORD_TIME.test(t)) ?? null,
        unread: mentionCount(row.label!),
      });
    }

    // Servers on the rail: their text channels.
    const servers = nodes.filter((n) => n.clickable && n.rect && n.rect.x < RAIL_WIDTH && n.rect.w <= RAIL_WIDTH && n.label
      && n.label !== 'Direct Messages' && n.label !== 'Add a Server' && !/\(direct message\)$/.test(n.label)
      && !/^Explore|^Download/.test(n.label));
    for (const server of servers.slice(0, 5)) {
      const serverName = server.label!.replace(/^\d+ mentions?, /, '').trim();
      const s = await discordRailItem(nav, await nav.read(), (n) => n.label === server.label);
      for (const ch of s.filter((n) => n.clickable && /\(text channel\)$/.test(n.label ?? ''))) {
        const name = ch.label!.replace(/^(unread, )?/, '').replace(/ \(text channel\)$/, '').replace(/^\d+ mentions?, /, '');
        out.push({ id: `${serverName}/${name}`, title: `#${name}`, group: serverName, time: null, preview: null, unread: mentionCount(ch.label!) });
      }
    }
    return out;
  },

  async open(nav, convo) {
    await discordHome(nav);
    if (convo.id.startsWith('dm:')) {
      const nodes = await discordRailItem(nav, await nav.read(), (n) => n.label === 'Direct Messages');
      const row = nodes.find((n) => n.clickable && (n.rect?.x ?? 0) > RAIL_WIDTH && /\(direct message\)$/.test(n.label ?? '')
        && nodesInside(nodes, n).some((t) => t.label === convo.title));
      if (!row) throw new ReaderError('not_found', `No Discord conversation with ${convo.title} is in the list.`);
      await nav.tapNode(row);
    } else {
      const [server, channel] = [convo.group, convo.title.replace(/^#/, '')];
      const isServer = (n: UiNode) => n.clickable && (n.rect?.x ?? 999) < RAIL_WIDTH && (n.rect?.w ?? 999) <= RAIL_WIDTH && (n.label ?? '').replace(/^\d+ mentions?, /, '').trim() === server;
      if (!(await nav.read()).some(isServer)) throw new ReaderError('not_found', `The ${server} server isn’t on Discord’s server list.`);
      await discordRailItem(nav, await nav.read(), isServer);
      if (!(await nav.tapLabel(`${channel} (text channel)`))) throw new ReaderError('not_found', `#${channel} isn’t in ${server}.`);
    }
    await nav.wait(1500);
  },

  messages(nodes, me) {
    const out: ChannelMessage[] = [];
    const composer = nodes.find((n) => n.editable);
    const top = 170;
    const bottom = composer?.rect?.y ?? Number.POSITIVE_INFINITY;
    // Discord's own action buttons (Send a wave, Invite…) are not message content.
    const buttons = nodes.filter((n) => n.type === 'Button' && n.rect);
    const inButton = (n: UiNode) => buttons.some((b) => b === n || nodesInside([n], b).length > 0);
    // A message's body is the accessories_view list: each child is a text
    // line or a block (embed card, attachment, sticker) shown as a picture.
    const lists = nodes.filter((n) => n.id?.endsWith('accessories_view') && n.rect);
    const blockOf = (n: UiNode) => lists.find((l) => n.depth === l.depth + 1 && nodesInside([n], l).length > 0);
    const underList = (n: UiNode) => lists.some((l) => n.depth > l.depth + 1 && nodesInside([n], l).length > 0);
    let current: { author: string; time: string | null } | null = null;
    const add = (part: MessagePart) => {
      if (!current) return;
      const last = out[out.length - 1];
      const msg = last && last.author === current.author && last.time === current.time
        ? last
        : (out.push({ author: current.author, time: current.time, text: '', mine: me !== null && current.author === me, parts: [] }), out[out.length - 1]);
      const line = part.kind === 'text' ? part.text : `[${part.alt.split('\n')[0].slice(0, 80) || 'Image'}]`;
      msg.text = msg.text ? `${msg.text}\n${line}` : line;
      msg.parts!.push(part);
    };
    const picture = (n: UiNode, alt: string) => {
      const r = n.rect!;
      const y0 = Math.max(r.y, top), y1 = Math.min(r.y + r.h, bottom);
      if (y1 - y0 < 24 || r.w < 24) return;
      add({ kind: 'image', alt, width: r.w, height: y1 - y0, crop: { x: r.x, y: y0, w: r.w, h: y1 - y0 } });
    };
    for (const n of nodes) {
      if (!n.rect || n.rect.y + n.rect.h <= top || n.rect.y >= bottom || inButton(n)) continue;
      const header = n.label ? DISCORD_HEADER.exec(n.label) : null;
      if (header && n.type !== 'TextView') {
        current = { author: header[1], time: header[2] };
        continue;
      }
      if (n.id?.endsWith('author_name') || n.id?.endsWith('timestamp') || n.id?.endsWith('middle_text')) continue;
      if (!current || underList(n)) continue;
      if (blockOf(n)) {
        if (n.type === 'TextView' && n.label) { if (n.label !== current.time) add({ kind: 'text', text: n.label }); continue; }
        const inner = nodesInside(nodes, n);
        if (!inner.some((c) => c.type === 'ImageView' || c.id?.endsWith('border')) && n.type !== 'ImageView') {
          for (const t of inner) if (t.type === 'TextView' && t.label) add({ kind: 'text', text: t.label });
          continue;
        }
        picture(n, inner.map((c) => c.label).filter((l): l is string => Boolean(l)).join('\n') || n.label || 'Image');
        continue;
      }
      if (!n.label) continue;
      if (n.type === 'TextView' && n.label !== current.time) add({ kind: 'text', text: n.label });
      else if (n.type === 'ImageView') picture(n, n.label);
    }
    return out;
  },

  async send(nav, text) {
    const nodes = await nav.read();
    const composer = nodes.find((n) => n.editable && (n.id ?? '').endsWith('chat_input_edit_text'));
    if (!composer) throw new ReaderError('no_composer', 'Discord isn’t showing a message box for this conversation.');
    await nav.tapNode(composer);
    const typed = await nav.call('phone.type', { text });
    if (!typed.ok) throw new ReaderError(typed.error ?? 'type_failed', typed.message ?? 'Couldn’t type into Discord.');
    await nav.wait(500);
    if (!(await nav.tapLabel('Send'))) throw new ReaderError('no_send', 'Discord didn’t show its Send button.');
    await nav.wait(800);
  },
};

export const CHANNEL_SPECS: Record<string, ChannelAppSpec> = { discord };

// ── Reader ──────────────────────────────────────────────────────────────────

/**
 * Pictures are kept only for each conversation's latest read (a new read
 * replaces the old one), and all of them together stay under this many bytes:
 * past it, the conversations read longest ago drop their pictures (their
 * text stays) until it fits. Opening one again re-reads them from the phone.
 */
export const PICTURE_CACHE_BYTES = 20 * 1024 * 1024;

interface Cache {
  conversations: Record<string, { data: ChannelConversation[]; readAt: string }>;
  messages: Record<string, { data: ChannelMessage[]; readAt: string }>;
  me: Record<string, string>;
  clips?: Record<string, ChannelClip>;
}

export interface ChannelReaderDeps {
  /** The phone tool runner (phone-service's PhoneToolRunner.call). */
  call: (name: string, args: unknown) => Promise<{ ok: boolean; error?: string; message?: string } & Record<string, unknown>>;
  /** Local copy (Desktop user data); never uploaded. */
  cacheFile: string;
  now?: () => Date;
  wait?: (ms: number) => Promise<void>;
  /** Cut `rect` (screen coordinates; the screen is `screenWidth` wide) out of a screenshot → data URL, or null. */
  crop?: (imageBase64: string, rect: Rect, screenWidth: number) => string | null;
}

/** Record the phone's screen for a few seconds → base64 MP4 at `size`, or null. */
export type ScreenRecorder = (size: { width: number; height: number }) => Promise<string | null>;

export class PhoneChannelReader {
  private queue: Promise<unknown> = Promise.resolve();
  private cache: Cache;

  constructor(private readonly deps: ChannelReaderDeps) {
    this.cache = this.load();
  }

  apps(): Array<{ app: string; name: string }> {
    return Object.values(CHANNEL_SPECS).map((s) => ({ app: s.app, name: s.name }));
  }

  /** The last copy read, without touching the phone. */
  cached(app: string, conversationId?: string): { conversations?: ChannelConversation[]; messages?: ChannelMessage[]; readAt?: string } {
    if (conversationId) {
      const m = this.cache.messages[`${app}::${conversationId}`];
      return m ? { messages: m.data, readAt: m.readAt } : {};
    }
    const c = this.cache.conversations[app];
    return c ? { conversations: c.data, readAt: c.readAt } : {};
  }

  conversations(serial: string, app: string): Promise<ReaderResult<ChannelConversation[]>> {
    return this.run(serial, app, async (spec, nav) => {
      const data = await spec.conversations(nav);
      const me = spec.me?.(await nav.read());
      if (me) this.cache.me[app] = me;
      this.cache.conversations[app] = { data, readAt: this.stamp() };
      return data;
    });
  }

  /**
   * Read a conversation. With `record` (the phone's screen recorder), a read
   * that finds animated parts records their clip right away — the phone is
   * already showing them — so they play without a tap (cachedClip).
   */
  messages(serial: string, app: string, conversation: ChannelConversation, record: ScreenRecorder | null = null): Promise<ReaderResult<ChannelMessage[]>> {
    return this.run(serial, app, async (spec, nav) => {
      await spec.open(nav, conversation);
      return this.readAndKeep(spec, nav, app, conversation, record);
    });
  }

  send(serial: string, app: string, conversation: ChannelConversation, text: string, record: ScreenRecorder | null = null): Promise<ReaderResult<ChannelMessage[]>> {
    const body = text.trim();
    if (!body || body.length > 2000) return Promise.resolve({ ok: false, error: 'invalid_args', message: 'A message is 1–2000 characters.' });
    return this.run(serial, app, async (spec, nav) => {
      await spec.open(nav, conversation);
      await spec.send(nav, body);
      return this.readAndKeep(spec, nav, app, conversation, record);
    });
  }

  /** Read the open conversation, keep it, and record a clip when something on it moves. */
  private async readAndKeep(spec: ChannelAppSpec, nav: ReaderNav, app: string, conversation: ChannelConversation, record: ScreenRecorder | null): Promise<ChannelMessage[]> {
    const key = `${app}::${conversation.id}`;
    const data = await this.readMessages(spec, nav, app);
    delete this.cache.clips?.[key];
    this.cache.messages[key] = { data, readAt: this.stamp() };
    const moving = data.some((m) => m.parts?.some((p) => p.kind === 'image' && p.animated));
    if (moving && record) {
      // A clip is a bonus: a failed recording leaves the stills.
      const clip = await this.recordScreen(nav, record).catch(() => null);
      if (clip) (this.cache.clips ??= {})[key] = clip;
    }
    return data;
  }

  /** Record a few seconds of what the phone shows now. */
  private async recordScreen(nav: ReaderNav, record: ScreenRecorder): Promise<ChannelClip> {
    const nodes = await nav.read();
    const screenWidth = Math.max(0, ...nodes.map((n) => (n.rect ? n.rect.x + n.rect.w : 0)));
    const screenHeight = Math.max(0, ...nodes.map((n) => (n.rect ? n.rect.y + n.rect.h : 0)));
    if (!screenWidth || !screenHeight) throw new ReaderError('read_failed', 'Couldn’t read the phone’s screen.');
    // Half size is plenty for a 280px-wide part; even numbers for the encoder.
    const even = (n: number) => Math.max(2, Math.round(n / 4) * 2);
    const b64 = await record({ width: even(screenWidth), height: even(screenHeight) });
    if (!b64) throw new ReaderError('record_failed', 'The phone didn’t record its screen. Try again.');
    return { src: b64.startsWith('data:') ? b64 : `data:video/mp4;base64,${b64}`, screenWidth, screenHeight, recordedAt: this.stamp() };
  }

  /** Read the open conversation; non-text parts become crops of one screenshot. */
  private async readMessages(spec: ChannelAppSpec, nav: ReaderNav, app: string): Promise<ChannelMessage[]> {
    const nodes = await nav.read();
    const data = spec.messages(nodes, this.cache.me[app] ?? null);
    const pictures = data.flatMap((m) => m.parts ?? []).filter((p): p is Extract<MessagePart, { kind: 'image' }> => p.kind === 'image');
    if (pictures.length && this.deps.crop) {
      const shoot = async () => {
        const shot = await nav.call('phone.screenshot', {}).catch(() => null);
        return shot?.ok && typeof shot.dataBase64 === 'string' ? shot.dataBase64 : null;
      };
      const b64 = await shoot();
      const screenWidth = Math.max(0, ...nodes.map((n) => (n.rect ? n.rect.x + n.rect.w : 0)));
      for (const p of pictures) {
        if (b64 && p.crop && screenWidth > 0) p.src = this.deps.crop(b64, p.crop, screenWidth) ?? undefined;
      }
      // A second look shortly after: a picture whose pixels changed is animated.
      if (b64 && screenWidth > 0) {
        await nav.wait(400);
        const again = await shoot();
        for (const p of pictures) {
          if (again && p.crop && p.src && this.deps.crop(again, p.crop, screenWidth) !== p.src) p.animated = true;
        }
      }
    }
    for (const p of pictures) { if (p.crop) p.at = p.crop; delete p.crop; }
    return data;
  }

  /** The last clip recorded for a conversation, without touching the phone. */
  cachedClip(app: string, conversationId: string): ChannelClip | null {
    return this.cache.clips?.[`${app}::${conversationId}`] ?? null;
  }

  /**
   * Record a few seconds of a conversation as the phone shows it, so its
   * animated parts (GIFs, stickers, videos) play in Allternit. Opens the
   * conversation first, the same way a read does.
   */
  clip(serial: string, app: string, conversation: ChannelConversation, record: ScreenRecorder | null): Promise<ReaderResult<ChannelClip>> {
    if (!record) return Promise.resolve({ ok: false, error: 'unsupported', message: 'Playing needs the phone’s screen connection (Settings › Computers › Phones).' });
    return this.run(serial, app, async (spec, nav) => {
      await spec.open(nav, conversation);
      const clip = await this.recordScreen(nav, record);
      (this.cache.clips ??= {})[`${app}::${conversation.id}`] = clip;
      return clip;
    });
  }

  /** One operation at a time: each one drives the phone's screen. */
  private run<T>(serial: string, app: string, op: (spec: ChannelAppSpec, nav: ReaderNav) => Promise<T>): Promise<ReaderResult<T>> {
    const spec = CHANNEL_SPECS[app];
    if (!spec) return Promise.resolve({ ok: false, error: 'unsupported_app', message: 'Allternit can’t read this app yet.' });
    const next = this.queue.then(async (): Promise<ReaderResult<T>> => {
      try {
        const data = await op(spec, this.nav(serial));
        this.save();
        return { ok: true, data, readAt: this.stamp() };
      } catch (error) {
        if (error instanceof ReaderError) return { ok: false, error: error.code, message: error.message };
        return { ok: false, error: 'reader_failed', message: error instanceof Error ? error.message : String(error) };
      }
    });
    this.queue = next.catch(() => undefined);
    return next;
  }

  private nav(serial: string): ReaderNav {
    const wait = this.deps.wait ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
    const call: PhoneCall = async (name, args) => {
      const res = await this.deps.call(name, { serial, ...args });
      if (!res.ok && (res.error === 'device_offline' || res.error === 'device_locked' || res.error === 'pairing_invalid')) {
        throw new ReaderError(res.error, res.message ?? 'The phone isn’t reachable.');
      }
      return res as Awaited<ReturnType<PhoneCall>>;
    };
    return {
      call,
      wait,
      read: async () => {
        const res = await call('phone.read_ui', { visible_only: true, max_elements: 400 });
        if (!res.ok || typeof res.text !== 'string') throw new ReaderError(res.error ?? 'read_failed', res.message ?? 'Couldn’t read the phone’s screen.');
        return parseUiTree(res.text);
      },
      tapNode: async (n) => {
        const c = centerOf(n);
        if (!c) throw new ReaderError('not_found', 'That item has no position on screen.');
        await call('phone.tap', { x: c.x, y: c.y });
      },
      tapLabel: async (label) => (await call('phone.find_and_tap', { element_id: label })).ok,
    };
  }

  private stamp(): string {
    return (this.deps.now?.() ?? new Date()).toISOString();
  }

  private load(): Cache {
    try {
      const raw = JSON.parse(fs.readFileSync(this.deps.cacheFile, 'utf8')) as Partial<Cache>;
      return { conversations: raw.conversations ?? {}, messages: raw.messages ?? {}, me: raw.me ?? {}, clips: raw.clips ?? {} };
    } catch {
      return { conversations: {}, messages: {}, me: {}, clips: {} };
    }
  }

  private save(): void {
    trimPictures(this.cache.messages, PICTURE_CACHE_BYTES, this.cache.clips);
    try {
      fs.mkdirSync(path.dirname(this.deps.cacheFile), { recursive: true });
      fs.writeFileSync(this.deps.cacheFile, JSON.stringify(this.cache), { mode: 0o600 });
    } catch {
      /* the copy is a convenience; reading still works */
    }
  }
}

/** Drop pictures from the least recently read conversations until the rest fit in `limit` bytes. */
export function trimPictures(messages: Cache['messages'], limit: number, clips: Record<string, ChannelClip> = {}): void {
  const pics = (m: ChannelMessage[]) => m.flatMap((x) => x.parts ?? []).filter((p): p is Extract<MessagePart, { kind: 'image' }> => p.kind === 'image' && Boolean(p.src));
  const entries = Object.values(messages).sort((a, b) => b.readAt.localeCompare(a.readAt));
  let total = 0;
  for (const e of entries) {
    for (const p of pics(e.data)) {
      total += p.src!.length;
      if (total > limit) delete p.src;
    }
  }
  // Clips share the cap: newest kept, the rest dropped (tap ▶ records again).
  for (const [key, c] of Object.entries(clips).sort((a, b) => b[1].recordedAt.localeCompare(a[1].recordedAt))) {
    total += c.src.length;
    if (total > limit) delete clips[key];
  }
}
