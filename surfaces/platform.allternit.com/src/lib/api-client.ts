/**
 * Minimal Allternit API client for the platform console.
 *
 * Uses the gateway at VITE_ALLTERNIT_GATEWAY_URL (default https://api.allternit.com).
 * Bearer tokens come from the Clerk token provider registered by
 * PlatformAuthProvider, called on every request (see TokenProvider).
 *
 * Phase 2 extension — `stream()`:
 * The `/v1/*` LLM gateway accepts `stream: true` on chat completions and
 * returns an SSE (`text/event-stream`) response. `stream()` POSTs a JSON body
 * through the same gateway base + token plumbing as `request()` and yields
 * each parsed `data:` payload as an async iterable. Pass an AbortSignal to
 * stop the stream (the console's Run/Stop button). HTTP errors are raised
 * before the first event, with the same error shape as `request()`.
 */

export class AllternitApiError extends Error {
  constructor(
    message: string,
    public statusCode: number,
    public code?: string,
    public details?: Record<string, unknown>
  ) {
    super(message);
    this.name = 'AllternitApiError';
  }

  isAuthError(): boolean {
    return this.statusCode === 401 || this.statusCode === 403;
  }
}

/**
 * cloud-api answers relayed (data-plane) routes with HTTP 428
 * PRECONDITION_REQUIRED when the account has no paired node. The client maps
 * that to `code: "no_runtime"` and this message, so any page that prints
 * `formatApiError(err)` shows a readable sentence instead of the raw code,
 * and pages can swap in <NoRuntimeState /> via `isNoRuntime(error)`.
 */
export const NO_RUNTIME_CODE = 'no_runtime';
export const NO_RUNTIME_MESSAGE =
  'No computer is connected. Pair your Mac, PC or a cloud computer to run agents, sessions and vaults.';

/** True for a 428 no-runtime error, or the string formatApiError made from one. */
export function isNoRuntime(err: unknown): boolean {
  if (err instanceof AllternitApiError) {
    return err.code === NO_RUNTIME_CODE || err.statusCode === 428;
  }
  if (typeof err === 'string') return err === NO_RUNTIME_MESSAGE;
  if (err instanceof Error) return err.message === NO_RUNTIME_MESSAGE;
  return false;
}

async function toApiError(response: Response): Promise<AllternitApiError> {
  const errorData = await response.json().catch(() => ({}));
  // Platform API errors nest as { error: { type, code, message, param } }.
  const nested = errorData.error && typeof errorData.error === 'object' ? errorData.error : null;
  const message =
    (nested ? nested.message : errorData.error) || errorData.message || `HTTP ${response.status}`;
  if (response.status === 428) {
    return new AllternitApiError(NO_RUNTIME_MESSAGE, 428, NO_RUNTIME_CODE, {
      ...(errorData.details ?? {}),
      server_message: message,
    });
  }
  return new AllternitApiError(
    message,
    response.status,
    (nested ? nested.code : undefined) ?? errorData.code,
    errorData.details
  );
}

export function formatApiError(err: unknown, fallback: string): string {
  if (err instanceof AllternitApiError) return err.message;
  if (err instanceof Error) {
    if (err.name === 'AbortError' || /abort/i.test(err.message)) {
      return 'Request timed out. The cloud API did not respond.';
    }
    return err.message;
  }
  return fallback;
}

/**
 * A token provider is called on every request. Clerk's `session.getToken()`
 * caches and refreshes the JWT itself, so the client never holds its own copy
 * (a held copy goes stale: Clerk tokens live 60 s and background tabs throttle
 * timers). `skipCache: true` asks for a newly minted token after a 401.
 */
export type TokenProvider = (options?: { skipCache?: boolean }) => Promise<string | null>;

function hasAuthorizationHeader(headers: Record<string, string>): boolean {
  return Object.keys(headers).some((key) => key.toLowerCase() === 'authorization');
}

export class AllternitApiClient {
  /** Static bearer set on purpose by a non-Clerk caller; used only when no provider is registered. */
  private token: string | null = null;
  private tokenProvider: TokenProvider | null = null;
  /** Resolves once a token or provider is registered, so early requests wait for auth. */
  private authReady: Promise<void> | null = null;
  private authReadyResolve: (() => void) | null = null;

  constructor() {
    this.resetAuthReady();
  }

  private resetAuthReady(): void {
    this.authReady = new Promise((resolve) => {
      this.authReadyResolve = resolve;
    });
  }

  private signalAuthReady(): void {
    if (this.authReadyResolve) {
      this.authReadyResolve();
      this.authReadyResolve = null;
    }
  }

  setToken(token: string): void {
    this.token = token;
    this.signalAuthReady();
  }

  clearToken(): void {
    this.token = null;
    if (!this.tokenProvider) this.resetAuthReady();
  }

  setTokenProvider(provider: TokenProvider): void {
    this.tokenProvider = provider;
    this.signalAuthReady();
  }

  clearTokenProvider(): void {
    this.tokenProvider = null;
    if (!this.token) {
      // Release anyone waiting so they fail fast instead of hanging, then
      // arm a new wait for the next sign-in.
      this.signalAuthReady();
      this.resetAuthReady();
    }
  }

  isAuthenticated(): boolean {
    return !!this.tokenProvider || !!this.token;
  }

  private gatewayBase(): string {
    const configured = import.meta.env.VITE_ALLTERNIT_GATEWAY_URL || 'https://api.allternit.com';
    return String(configured).replace(/\/+$/, '');
  }

  private async currentToken(): Promise<string | null> {
    if (this.tokenProvider) {
      try {
        return await this.tokenProvider();
      } catch {
        return null;
      }
    }
    return this.token;
  }

  private async resolveToken(): Promise<string | null> {
    if (this.tokenProvider || this.token) return this.currentToken();
    // The auth provider may not have mounted yet. Wait briefly for a token
    // or provider to be registered before sending an unauthenticated request.
    if (this.authReady) {
      const timeoutMs = Number(import.meta.env.VITE_ALLTERNIT_AUTH_TIMEOUT_MS || 3000);
      let timer: ReturnType<typeof setTimeout> | undefined;
      const timeout = new Promise<void>((resolve) => {
        timer = setTimeout(resolve, timeoutMs);
      });
      await Promise.race([this.authReady, timeout]);
      if (timer !== undefined) clearTimeout(timer);
      return this.currentToken();
    }
    return null;
  }

  /**
   * fetch with the session bearer. A caller-supplied Authorization header
   * (e.g. an `ak-…` virtual key) is sent as-is and never retried. When the
   * bearer came from the token provider and the server answers 401, retry
   * once with a freshly minted token before surfacing the error.
   */
  private async authedFetch(
    url: string,
    init: RequestInit,
    baseHeaders: Record<string, string>
  ): Promise<Response> {
    const callerHeaders = (init.headers as Record<string, string>) || {};
    const send = (token: string | null) =>
      fetch(url, {
        ...init,
        headers: {
          ...baseHeaders,
          ...(token ? { Authorization: `Bearer ${token}` } : {}),
          ...callerHeaders,
        },
      });

    if (hasAuthorizationHeader(callerHeaders)) return send(null);

    const token = await this.resolveToken();
    const response = await send(token);
    const provider = this.tokenProvider;
    if (response.status !== 401 || !token || !provider) return response;

    let fresh: string | null = null;
    try {
      fresh = await provider({ skipCache: true });
    } catch {
      fresh = null;
    }
    if (!fresh) return response;
    response.body?.cancel().catch(() => undefined);
    return send(fresh);
  }

  private async request<T>(
    method: string,
    path: string,
    body?: unknown,
    options: RequestInit = {}
  ): Promise<T> {
    const normalizedPath = path.startsWith('/') ? path : `/${path}`;

    const controller = new AbortController();
    const timeoutMs = Number(import.meta.env.VITE_ALLTERNIT_API_TIMEOUT_MS || 15000);
    const timeoutId = setTimeout(() => controller.abort(), timeoutMs);

    const response = await this.authedFetch(
      `${this.gatewayBase()}${normalizedPath}`,
      {
        ...options,
        method,
        signal: controller.signal,
        ...(body && method !== 'GET' ? { body: JSON.stringify(body) } : {}),
      },
      { 'Content-Type': 'application/json', Accept: 'application/json' }
    ).finally(() => clearTimeout(timeoutId));

    if (!response.ok) {
      throw await toApiError(response);
    }

    if (response.status === 204) {
      return undefined as T;
    }

    return response.json();
  }

  get<T>(path: string, options?: RequestInit): Promise<T> {
    return this.request<T>('GET', path, undefined, options);
  }

  post<T>(path: string, body?: unknown, options?: RequestInit): Promise<T> {
    return this.request<T>('POST', path, body, options);
  }

  patch<T>(path: string, body?: unknown, options?: RequestInit): Promise<T> {
    return this.request<T>('PATCH', path, body, options);
  }

  put<T>(path: string, body?: unknown, options?: RequestInit): Promise<T> {
    return this.request<T>('PUT', path, body, options);
  }

  delete<T>(path: string, options?: RequestInit): Promise<T> {
    return this.request<T>('DELETE', path, undefined, options);
  }

  async raw(path: string, options: RequestInit = {}): Promise<Response> {
    const normalizedPath = path.startsWith('/') ? path : `/${path}`;
    return this.authedFetch(`${this.gatewayBase()}${normalizedPath}`, options, {
      Accept: 'application/json',
    });
  }

  /**
   * POST `body` as JSON (or GET when `options.method: "GET"`) and yield each
   * SSE `data:` payload, parsed.
   * Callers pass an explicit bearer override via `options.headers.Authorization`
   * when targeting virtual-key routes (`/v1/*`) with an `ak-…` key.
   * The returned generator throws AllternitApiError on a non-OK response and
   * stops when the stream ends or `[DONE]` is received. Aborting `signal`
   * terminates the stream with an AbortError.
   */
  async *stream<T = unknown>(
    path: string,
    body?: unknown,
    options: RequestInit = {}
  ): AsyncGenerator<T> {
    const normalizedPath = path.startsWith('/') ? path : `/${path}`;
    const method = (options.method as string | undefined) ?? 'POST';
    const response = await this.authedFetch(
      `${this.gatewayBase()}${normalizedPath}`,
      {
        ...options,
        method,
        body:
          method === 'GET' || body === undefined
            ? undefined
            : JSON.stringify(body),
        signal: options.signal ?? null,
      },
      {
        Accept: 'text/event-stream',
        ...(method === 'GET' ? {} : { 'Content-Type': 'application/json' }),
      }
    );

    if (!response.ok || !response.body) {
      throw await toApiError(response);
    }

    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let buffer = '';
    try {
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        let newline: number;
        while ((newline = buffer.indexOf('\n')) >= 0) {
          const line = buffer.slice(0, newline).trim();
          buffer = buffer.slice(newline + 1);
          if (!line.startsWith('data:')) continue;
          const payload = line.slice(5).trim();
          if (payload === '[DONE]') return;
          try {
            yield JSON.parse(payload) as T;
          } catch {
            // Skip malformed keep-alive/comment payloads rather than killing the stream.
          }
        }
      }
    } finally {
      reader.cancel().catch(() => undefined);
    }
  }
}

export const api = new AllternitApiClient();
