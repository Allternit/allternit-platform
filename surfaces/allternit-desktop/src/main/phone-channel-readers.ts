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

export interface ChannelMessage {
  author: string;
  time: string | null;
  text: string;
  mine: boolean;
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
const DISCORD_HEADER = /^(.+), (\d{2}\/\d{2}\/\d{4} \d{1,2}:\d{2} [AP]M|Today at .+|Yesterday at .+)$/;
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
    const bottom = composer?.rect?.y ?? Number.POSITIVE_INFINITY;
    // Discord's own action buttons (Send a wave, Invite…) are not message content.
    const buttons = nodes.filter((n) => n.type === 'Button' && n.rect);
    const inButton = (n: UiNode) => buttons.some((b) => b === n || nodesInside([n], b).length > 0);
    let current: { author: string; time: string | null } | null = null;
    for (const n of nodes) {
      if (!n.rect || n.rect.y >= bottom || n.rect.y < 170 || inButton(n)) continue;
      const header = n.label ? DISCORD_HEADER.exec(n.label) : null;
      if (header && n.type !== 'TextView') {
        current = { author: header[1], time: header[2] };
        continue;
      }
      if (n.id?.endsWith('author_name') || n.id?.endsWith('timestamp') || n.id?.endsWith('middle_text')) continue;
      if (!current || !n.label) continue;
      const text = n.type === 'ImageView' ? `[${n.label}]` : n.type === 'TextView' ? n.label : null;
      if (!text || text === current.time) continue;
      const last = out[out.length - 1];
      if (last && last.author === current.author && last.time === current.time) last.text += `\n${text}`;
      else out.push({ author: current.author, time: current.time, text, mine: me !== null && current.author === me });
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

interface Cache {
  conversations: Record<string, { data: ChannelConversation[]; readAt: string }>;
  messages: Record<string, { data: ChannelMessage[]; readAt: string }>;
  me: Record<string, string>;
}

export interface ChannelReaderDeps {
  /** The phone tool runner (phone-service's PhoneToolRunner.call). */
  call: (name: string, args: unknown) => Promise<{ ok: boolean; error?: string; message?: string } & Record<string, unknown>>;
  /** Local copy (Desktop user data); never uploaded. */
  cacheFile: string;
  now?: () => Date;
  wait?: (ms: number) => Promise<void>;
}

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

  messages(serial: string, app: string, conversation: ChannelConversation): Promise<ReaderResult<ChannelMessage[]>> {
    return this.run(serial, app, async (spec, nav) => {
      await spec.open(nav, conversation);
      const data = spec.messages(await nav.read(), this.cache.me[app] ?? null);
      this.cache.messages[`${app}::${conversation.id}`] = { data, readAt: this.stamp() };
      return data;
    });
  }

  send(serial: string, app: string, conversation: ChannelConversation, text: string): Promise<ReaderResult<ChannelMessage[]>> {
    const body = text.trim();
    if (!body || body.length > 2000) return Promise.resolve({ ok: false, error: 'invalid_args', message: 'A message is 1–2000 characters.' });
    return this.run(serial, app, async (spec, nav) => {
      await spec.open(nav, conversation);
      await spec.send(nav, body);
      const data = spec.messages(await nav.read(), this.cache.me[app] ?? null);
      this.cache.messages[`${app}::${conversation.id}`] = { data, readAt: this.stamp() };
      return data;
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
      return { conversations: raw.conversations ?? {}, messages: raw.messages ?? {}, me: raw.me ?? {} };
    } catch {
      return { conversations: {}, messages: {}, me: {} };
    }
  }

  private save(): void {
    try {
      fs.mkdirSync(path.dirname(this.deps.cacheFile), { recursive: true });
      fs.writeFileSync(this.deps.cacheFile, JSON.stringify(this.cache), { mode: 0o600 });
    } catch {
      /* the copy is a convenience; reading still works */
    }
  }
}
