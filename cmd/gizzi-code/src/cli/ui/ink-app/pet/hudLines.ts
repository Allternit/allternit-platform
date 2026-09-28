import { messageHandoff, type ThreadMessage } from '@/runtime/bots/platform-threads.js';
import type { ChatLine } from './localChat';

/**
 * The pet HUD's Thread lines: chat lines and rips (where the thread moved to
 * a fresh context window, spec P3.16). A rip remembers the window it
 * continued from, so scrolling up past it can read that window in place,
 * the way Desktop's rip offers "Show the earlier conversation".
 */
export type HudRip = { role: 'rip'; generation?: number; reason?: string; at?: string; from?: string };
export type HudLine = ChatLine | HudRip;

export const isChat = (l: HudLine): l is ChatLine => l.role !== 'rip';

/** A window's messages as HUD lines; its seed (checkpoint) is drawn as the rip. */
export function hudLinesFromMessages(messages: ThreadMessage[]): HudLine[] {
  return messages.flatMap((m): HudLine[] => {
    const handoff = messageHandoff(m);
    if (handoff) return [{ role: 'rip', generation: (handoff.generation ?? 1) + 1, reason: handoff.reason, at: m.timestamp, from: handoff.from }];
    if ((m.role !== 'user' && m.role !== 'assistant') || !m.content || m.content === '[No text content]') return [];
    return [{ role: m.role, content: m.content }];
  });
}

/** The `shown` lines ending `offset` lines above the newest. */
export function visibleLines(lines: HudLine[], offset: number, shown: number): HudLine[] {
  const end = Math.max(0, lines.length - offset);
  return lines.slice(Math.max(0, end - shown), end);
}

/** Largest useful scroll offset: the oldest line at the top. */
export function maxOffset(lines: HudLine[], shown: number): number {
  return Math.max(0, lines.length - shown);
}

/**
 * The window to load when scrolling past the top: the oldest line is a rip
 * whose earlier window isn't loaded yet. null when there's nothing earlier.
 */
export function earlierWindowAtTop(lines: HudLine[], loaded: ReadonlySet<string>): string | null {
  const first = lines[0];
  return first && !isChat(first) && first.from && !loaded.has(first.from) ? first.from : null;
}
