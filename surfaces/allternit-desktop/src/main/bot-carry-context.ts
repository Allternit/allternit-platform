// bot-carry-context.ts — what a bot dropped out of the app is sitting on.
//
// When a bot is carried out of the app (or its pet is moved) and lands on
// another app's window, Quick Work gets that window as context: the app name
// and window title from `window-below.ts`, plus a picture of that one window.
//
// Privacy contract: the picture is taken only when the Screen Recording
// permission is already granted (never prompted from here), only of the one
// window under the pet, and it goes to the HUD as a removable attachment —
// nothing leaves the Mac unless the user sends the message.

import { desktopCapturer, systemPreferences } from 'electron';

import { readWindowBelow } from './window-below.js';

export interface BotCarryContext {
  /** Changes each time; the HUD replaces its previous context with this one. */
  seq: number;
  app: string;
  title: string;
  /** PNG data URL of the window, when Screen Recording is allowed. */
  image?: string;
  /** From a pet offer's Fix it: the error text, attached as a removable chip. Never logged. */
  text?: string;
  /** From a pet offer: the bubble line, used as the prefilled draft. Not sent until the user sends. */
  intent?: string;
  /** From a pet offer read off another app: turns sent with this attachment carry metadata.tools.computer = true. */
  computer?: boolean;
  /** From a pet offer: its kind (build_failed, stack_trace, error_dialog, run_failed). */
  kind?: string;
}

/** Longest side of the picture: enough to read a chart, small enough to send. */
const MAX_EDGE = 1600;

const screenAllowed = (): boolean =>
  process.platform !== 'darwin' || systemPreferences.getMediaAccessStatus('screen') === 'granted';

/** Capture the one window `id` (the OS window id from window-below), or nothing. */
async function captureWindow(id: number, bounds: { width: number; height: number }): Promise<string | undefined> {
  if (!id || !bounds.width || !bounds.height) return undefined;
  const scale = Math.min(1, MAX_EDGE / Math.max(bounds.width, bounds.height)) * 2;
  const thumbnailSize = { width: Math.round(bounds.width * scale), height: Math.round(bounds.height * scale) };
  try {
    const sources = await desktopCapturer.getSources({ types: ['window'], thumbnailSize, fetchWindowIcons: false });
    const source = sources.find(s => s.id.startsWith(`window:${id}:`));
    if (!source || source.thumbnail.isEmpty()) return undefined;
    const size = source.thumbnail.getSize();
    const fit = Math.min(1, MAX_EDGE / Math.max(size.width, size.height));
    const image = fit < 1 ? source.thumbnail.resize({ width: Math.round(size.width * fit), quality: 'good' }) : source.thumbnail;
    return image.toDataURL();
  } catch {
    return undefined;
  }
}

let seq = 0;

/** The window under `petBounds`, or null when none (desktop, own window) or enumeration can't run. */
export async function readBotCarryContext(petBounds: { x: number; y: number; width: number; height: number }): Promise<BotCarryContext | null> {
  const allowed = screenAllowed();
  // Only the centre of the pet counts: a pet straddling two windows sits on the one under its middle.
  const centre = { x: Math.round(petBounds.x + petBounds.width / 2), y: Math.round(petBounds.y + petBounds.height / 2), width: 1, height: 1 };
  const result = await readWindowBelow(process.pid, centre, allowed);
  if ('error' in result || !result.window) return null;
  const { window } = result;
  return {
    seq: ++seq,
    app: window.app || 'App',
    title: window.title,
    image: allowed ? await captureWindow(window.id, window.bounds) : undefined,
  };
}

/** Quick Work context for a pet offer the user accepted (desktop-companion.ts acceptOffer). */
export function offerCarryContext(offer: { app: string; title: string; text: string; intent: string; computer: boolean; kind: string }): BotCarryContext {
  return {
    seq: ++seq,
    app: offer.app || 'App',
    title: offer.title,
    ...(offer.text ? { text: offer.text } : {}),
    intent: offer.intent,
    computer: offer.computer,
    kind: offer.kind,
  };
}
