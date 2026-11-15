// pet-notice.ts — the Notice step of the desktop pet's offer loop: plain rules
// from the shared pet-offers contract, and the gate that decides whether an
// offer may show. Pure: no Electron, no I/O, no clock of its own.
//
// Matcher (contract matcher.steps): \r\n and \r become \n; only the last
// maxTextChars characters count; rules in file order, "dialog" rules only for
// text from a dialog; the first match wins; the line is the whole line holding
// the start of the match, trimmed, cut to limits.lineMaxChars.
//
// Gate (plan decision 11 + the switches): offers on, reading on for passive
// reads, app not muted, one offer per window per cooldown, and never the same
// matched line twice for one window.
//
// Privacy: the gate keeps a hash of each matched line, never the line.

import {
  PET_OFFER_COPY,
  PET_OFFER_LIMITS,
  PET_OFFER_RULES,
  type PetOfferKind,
  type PetOfferSettings,
  type PetOfferTrigger,
} from './pet-offers.gen.js';

const COMPILED = PET_OFFER_RULES.map(rule => ({ rule, re: new RegExp(rule.pattern, rule.flags) }));

/** Contract normalization: one newline style, only the tail counts. */
export function normalizeOfferText(text: string, maxTextChars: number): string {
  const flat = text.replace(/\r\n?/g, '\n');
  return flat.length > maxTextChars ? flat.slice(flat.length - maxTextChars) : flat;
}

/** The first contract rule that matches, or null (no offer). */
export function matchOffer(text: string, options: { dialog?: boolean; maxTextChars: number }): PetOfferTrigger | null {
  if (!text) return null;
  const body = normalizeOfferText(text, options.maxTextChars);
  for (const { rule, re } of COMPILED) {
    if (rule.scope === 'dialog' && !options.dialog) continue;
    const m = re.exec(body);
    if (!m) continue;
    const start = body.lastIndexOf('\n', m.index - 1) + 1;
    const endAt = body.indexOf('\n', m.index);
    const line = body.slice(start, endAt === -1 ? body.length : endAt).trim().slice(0, PET_OFFER_LIMITS.lineMaxChars);
    return { kind: rule.kind, ruleId: rule.id, line };
  }
  return null;
}

/** The bubble line for a kind ("{App} is showing an error. Fix it?"). */
export function offerCopy(kind: PetOfferKind, app: string): string {
  return PET_OFFER_COPY.byKind[kind].replace(PET_OFFER_COPY.appPlaceholder, app.trim() || PET_OFFER_COPY.appFallback);
}

/** "Mute Terminal" */
export function offerMuteLabel(app: string): string {
  return PET_OFFER_COPY.actions.mute.replace(PET_OFFER_COPY.appPlaceholder, app.trim() || PET_OFFER_COPY.appFallback);
}

/** True when `entries` names this app, by name first, then by id (case-insensitive, exact). */
export function listNamesApp(entries: readonly string[], app: string, appId: string): boolean {
  const name = app.trim().toLowerCase();
  const id = appId.trim().toLowerCase();
  return entries.some(entry => {
    const e = entry.trim().toLowerCase();
    return e.length > 0 && (e === name || e === id);
  });
}

/** 32-bit FNV-1a: enough to tell matched lines apart without keeping them. */
function lineHash(line: string): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < line.length; i++) {
    h ^= line.charCodeAt(i);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h;
}

export type OfferSource = 'look' | 'hook' | 'app';

export interface OfferCandidate {
  source: OfferSource;
  app: string;
  appId: string;
  windowKey: string;
  line: string;
}

export type GateVerdict = 'ok' | 'off' | 'reading_off' | 'muted' | 'cooldown' | 'same_line';

/** Remembers which windows were offered on, and when, and which lines. Bounded. */
export class OfferGate {
  private windows = new Map<string, { at: number; lines: Set<number> }>();

  constructor(private readonly maxWindows = 200) {}

  check(candidate: OfferCandidate, settings: PetOfferSettings, now: number): GateVerdict {
    if (!settings.enabled) return 'off';
    if (candidate.source === 'look' && !settings.readOtherApps) return 'reading_off';
    if (listNamesApp(settings.muted, candidate.app, candidate.appId)) return 'muted';
    const seen = this.windows.get(candidate.windowKey);
    if (seen) {
      // An event offer with no line (a hook "failed") only has the cooldown.
      if (candidate.line && seen.lines.has(lineHash(candidate.line))) return 'same_line';
      if (now - seen.at < settings.cooldownMs) return 'cooldown';
    }
    return 'ok';
  }

  /** Call when an offer actually shows. */
  record(candidate: OfferCandidate, now: number): void {
    const seen = this.windows.get(candidate.windowKey) ?? { at: now, lines: new Set<number>() };
    seen.at = now;
    if (candidate.line) seen.lines.add(lineHash(candidate.line));
    // Re-insert so the map's order is least recently offered first.
    this.windows.delete(candidate.windowKey);
    this.windows.set(candidate.windowKey, seen);
    while (this.windows.size > this.maxWindows) this.windows.delete(this.windows.keys().next().value as string);
  }
}

/** Contract windowKey: appId + "|" + title. */
export function offerWindowKey(appId: string, title: string): string {
  return `${appId}|${title}`;
}
