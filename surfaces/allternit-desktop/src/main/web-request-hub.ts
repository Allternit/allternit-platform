/**
 * webRequest hub.
 *
 * Electron keeps ONE listener per webRequest event per session: a second
 * `session.webRequest.onBeforeRequest(...)` silently replaces the first, and
 * `(null)` removes whatever is there. Before this hub, starting a Teach/API
 * capture replaced the main window's API routing + auth header listeners and
 * stopping it removed them, and the OAuth popup did the same to
 * onBeforeRequest. Everything in main now registers through `onWebRequest`,
 * which installs one real listener per (session, event) and runs every
 * registered handler in order.
 *
 * Blocking events chain: a handler's `cancel` or `redirectURL` ends the chain,
 * `requestHeaders` / `responseHeaders` carry forward to the next handler, and
 * the merged result goes to Electron. Observers just call `callback({})`.
 */

import type { Session } from 'electron';
import log from 'electron-log';

type BlockingEvent = 'onBeforeRequest' | 'onBeforeSendHeaders' | 'onHeadersReceived';
type PassiveEvent = 'onCompleted' | 'onErrorOccurred';
export type WebRequestEvent = BlockingEvent | PassiveEvent;

// Electron's per-event response types (CallbackResponse, BeforeSendResponse,
// HeadersReceivedResponse) differ; the hub only reads the shared fields.
type Callback = (response: any) => void;
type BlockingFn = (details: any, callback: Callback) => void;
type PassiveFn = (details: any) => void;

interface Entry {
  id: number;
  urls?: RegExp[];
  fn: BlockingFn | PassiveFn;
}

const BLOCKING = new Set<WebRequestEvent>(['onBeforeRequest', 'onBeforeSendHeaders', 'onHeadersReceived']);
const registry = new Map<Session, Map<WebRequestEvent, Entry[]>>();
let nextId = 1;

/** Electron URL patterns (`*://host/path*`) as regexes. */
function patternToRegExp(pattern: string): RegExp {
  const escaped = pattern.replace(/[.+?^${}()|[\]\\]/g, '\\$&').replace(/\*/g, '.*');
  return new RegExp(`^${escaped}$`);
}

function matches(entry: Entry, url: string): boolean {
  return !entry.urls || entry.urls.some((re) => re.test(url));
}

function install(sess: Session, event: WebRequestEvent, entries: Entry[]): void {
  const wr = sess.webRequest as any;
  if (!BLOCKING.has(event)) {
    wr[event]((details: any) => {
      for (const entry of entries.slice()) {
        if (!matches(entry, details.url)) continue;
        try {
          (entry.fn as PassiveFn)(details);
        } catch (err) {
          log.warn(`[WebRequestHub] ${event} handler failed:`, err);
        }
      }
    });
    return;
  }
  wr[event]((details: any, callback: Callback) => {
    const chain = entries.filter((entry) => matches(entry, details.url));
    const merged: Record<string, unknown> = {};
    let index = 0;
    const next = () => {
      const entry = chain[index++];
      if (!entry) {
        callback(merged);
        return;
      }
      let called = false;
      const step: Callback = (response = {}) => {
        if (called) return;
        called = true;
        if (response.cancel || response.redirectURL) {
          callback({ ...merged, ...response });
          return;
        }
        if (response.requestHeaders) {
          details.requestHeaders = response.requestHeaders;
          merged.requestHeaders = response.requestHeaders;
        }
        if (response.responseHeaders) {
          details.responseHeaders = response.responseHeaders;
          merged.responseHeaders = response.responseHeaders;
        }
        if (response.statusLine) merged.statusLine = response.statusLine;
        next();
      };
      try {
        (entry.fn as BlockingFn)(details, step);
      } catch (err) {
        log.warn(`[WebRequestHub] ${event} handler failed:`, err);
        step({});
      }
    };
    next();
  });
}

/**
 * Register a handler for one webRequest event on a session. Returns a
 * function that removes just this handler; the real Electron listener stays.
 */
export function onWebRequest(
  sess: Session,
  event: BlockingEvent,
  fn: BlockingFn,
  options?: { urls?: string[] },
): () => void;
export function onWebRequest(
  sess: Session,
  event: PassiveEvent,
  fn: PassiveFn,
  options?: { urls?: string[] },
): () => void;
export function onWebRequest(
  sess: Session,
  event: WebRequestEvent,
  fn: BlockingFn | PassiveFn,
  options?: { urls?: string[] },
): () => void {
  let events = registry.get(sess);
  if (!events) {
    events = new Map();
    registry.set(sess, events);
  }
  let entries = events.get(event);
  if (!entries) {
    entries = [];
    events.set(event, entries);
    install(sess, event, entries);
  }
  const entry: Entry = { id: nextId++, fn, urls: options?.urls?.map(patternToRegExp) };
  entries.push(entry);
  return () => {
    const list = registry.get(sess)?.get(event);
    if (!list) return;
    const at = list.findIndex((e) => e.id === entry.id);
    if (at >= 0) list.splice(at, 1);
  };
}

/** Test-only: forget installed listeners so each test gets a fresh hub. */
export function __resetWebRequestHubForTests(): void {
  registry.clear();
}
