/**
 * Clerk session-token cache for Electron main that never makes a caller wait
 * out a slow refresh.
 *
 * A refresh loads a hidden auth window and can take up to its 20s timeout.
 * Before this, every caller of getClerkToken awaited that attempt: the
 * renderer's fetch interceptor (every runtime API call, via IPC) and the
 * allternit-api://cloud protocol. When the hidden refresh kept failing, the
 * whole app's network froze for 20s every 80s (20s attempt + 60s backoff),
 * which timed out the Bots list at launch and left users on local-only bots.
 *
 * Rules:
 * - a fresh token is returned immediately;
 * - a stale-but-unexpired token is returned immediately and refreshed in the
 *   background (stale-while-revalidate);
 * - with no usable token, a caller waits at most `maxWaitMs` for a refresh;
 * - failures back off exponentially (60s → 15min) and reset on success.
 */

export interface ClerkTokenBrokerOptions {
  /** Performs one refresh; resolves the new token or rejects. */
  refresh: () => Promise<string>;
  /** Expiry (epoch ms) of a token, or null when it can't be read. */
  expiresAt: (token: string) => number | null;
  now?: () => number;
  /** Refresh this long before expiry. */
  refreshSkewMs?: number;
  /** Assumed lifetime when a token's expiry can't be read. */
  defaultLifetimeMs?: number;
  baseBackoffMs?: number;
  maxBackoffMs?: number;
  /** Called after each failed refresh (for logging / window cleanup). */
  onFailure?: (error: unknown, retryInMs: number) => void;
}

const DEFAULT_SKEW_MS = 10_000;
const DEFAULT_LIFETIME_MS = 50_000;
const DEFAULT_BASE_BACKOFF_MS = 60_000;
const DEFAULT_MAX_BACKOFF_MS = 15 * 60_000;

export class ClerkTokenBroker {
  private cache: { token: string; expiresAt: number } | null = null;
  private inFlight: Promise<string | null> | null = null;
  private failureUntil = 0;
  private failures = 0;
  private readonly now: () => number;

  constructor(private readonly options: ClerkTokenBrokerOptions) {
    this.now = options.now ?? Date.now;
  }

  /** Store a token obtained elsewhere (pairing, email recovery). */
  remember(token: string): void {
    if (!token) return;
    this.failures = 0;
    this.failureUntil = 0;
    this.cache = {
      token,
      expiresAt: this.options.expiresAt(token) ?? this.now() + (this.options.defaultLifetimeMs ?? DEFAULT_LIFETIME_MS),
    };
  }

  cachedToken(): string | null {
    return this.cache?.token ?? null;
  }

  /**
   * The current token without waiting on a slow refresh. `maxWaitMs` bounds
   * the wait only when there is no usable token at all.
   */
  async get(maxWaitMs: number): Promise<string | null> {
    const now = this.now();
    const skew = this.options.refreshSkewMs ?? DEFAULT_SKEW_MS;
    if (this.cache && this.cache.expiresAt - skew > now) return this.cache.token;

    const pending = this.startRefresh();
    // Stale but still valid: use it now, the refresh lands in the background.
    if (this.cache && this.cache.expiresAt > now) return this.cache.token;
    if (!pending || maxWaitMs <= 0) return null;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const bounded = new Promise<null>((resolve) => {
      timer = setTimeout(() => resolve(null), maxWaitMs);
    });
    try {
      return await Promise.race([pending, bounded]);
    } finally {
      if (timer) clearTimeout(timer);
    }
  }

  private startRefresh(): Promise<string | null> | null {
    if (this.inFlight) return this.inFlight;
    if (this.now() < this.failureUntil) return null;
    this.inFlight = this.options
      .refresh()
      .then((token) => {
        this.remember(token);
        return token;
      })
      .catch((error) => {
        this.failures += 1;
        const base = this.options.baseBackoffMs ?? DEFAULT_BASE_BACKOFF_MS;
        const max = this.options.maxBackoffMs ?? DEFAULT_MAX_BACKOFF_MS;
        const retryInMs = Math.min(base * 2 ** (this.failures - 1), max);
        this.failureUntil = this.now() + retryInMs;
        this.options.onFailure?.(error, retryInMs);
        return null;
      })
      .finally(() => {
        this.inFlight = null;
      });
    return this.inFlight;
  }
}
