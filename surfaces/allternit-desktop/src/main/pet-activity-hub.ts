/**
 * What the desktop pet shows, combined from every source that reports agent
 * activity: the app window (its pet-activity signal) and, later, terminal
 * agents through the loopback hook port. Mirrors the app's priority order
 * (allternit-ai src/lib/agents/pet-activity.ts).
 */
export const PET_STATES = [
  'idle', 'running-right', 'running-left', 'waving', 'jumping', 'failed',
  'waiting', 'running', 'review', 'sleep', 'special',
] as const;
export type PetState = (typeof PET_STATES)[number];

const PRIORITY: readonly PetState[] = ['failed', 'waiting', 'running', 'review', 'jumping', 'waving'];

export function isPetState(value: unknown): value is PetState {
  return typeof value === 'string' && (PET_STATES as readonly string[]).includes(value);
}

export class PetActivityHub {
  private sources = new Map<string, { state: PetState; until?: number }>();

  /** Set a source's state; `ttlMs` makes it expire (a source that may vanish without saying so). */
  set(source: string, state: PetState, ttlMs?: number, now = Date.now()): void {
    if (state === 'idle') this.sources.delete(source);
    else this.sources.set(source, { state, until: ttlMs ? now + ttlMs : undefined });
  }

  /** One source's live state (idle when it has none or it expired). */
  get(source: string, now = Date.now()): PetState {
    const s = this.sources.get(source);
    return s && (s.until === undefined || s.until > now) ? s.state : 'idle';
  }

  clear(source: string): void {
    this.sources.delete(source);
  }

  /** The most important live state; asleep only when every source is idle or asleep. */
  current(now = Date.now()): PetState {
    const live = new Set<PetState>();
    for (const [key, s] of this.sources) {
      if (s.until !== undefined && s.until <= now) { this.sources.delete(key); continue; }
      live.add(s.state);
    }
    for (const s of PRIORITY) if (live.has(s)) return s;
    return live.has('sleep') ? 'sleep' : 'idle';
  }

  /** Earliest moment a timed source expires, so the caller can recompute then. */
  nextExpiry(): number | undefined {
    let next: number | undefined;
    for (const s of this.sources.values()) if (s.until !== undefined) next = next === undefined ? s.until : Math.min(next, s.until);
    return next;
  }
}

/** Where the pet walks next: a point on its row at least 120 px away, inside the work area. */
export function pickRoamTarget(area: { x: number; width: number }, petWidth: number, currentX: number, rand = Math.random): number {
  const low = area.x + 8;
  const high = area.x + area.width - petWidth - 8;
  if (high - low < 240) return currentX;
  for (let i = 0; i < 8; i++) {
    const x = Math.round(low + rand() * (high - low));
    if (Math.abs(x - currentX) >= 120) return x;
  }
  return currentX - low > high - currentX ? low : high;
}

/** One step toward the target at `speed` px/s. */
export function roamStep(x: number, target: number, dtMs: number, speed = 70): number {
  const step = (speed * dtMs) / 1000;
  return Math.abs(target - x) <= step ? target : x + Math.sign(target - x) * step;
}
