/**
 * Minimal Allternit API client for the platform console.
 *
 * Uses the gateway at VITE_ALLTERNIT_GATEWAY_URL (default https://api.allternit.com).
 * Bearer tokens are set from the Clerk session sync in PlatformAuthProvider.
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

class AllternitApiClient {
  private token: string | null = null;
  private tokenProvider: (() => Promise<string | null>) | null = null;
  private authPromise: Promise<string | null> | null = null;
  private authResolve: ((token: string | null) => void) | null = null;

  constructor() {
    this.resetAuthPromise();
  }

  private resetAuthPromise(): void {
    this.authPromise = new Promise((resolve) => {
      this.authResolve = resolve;
    });
  }

  private flushAuth(token: string | null): void {
    if (this.authResolve) {
      this.authResolve(token);
      this.authResolve = null;
    }
  }

  setToken(token: string): void {
    this.token = token;
    this.flushAuth(token);
  }

  clearToken(): void {
    this.token = null;
    this.resetAuthPromise();
  }

  setTokenProvider(provider: () => Promise<string | null>): void {
    this.tokenProvider = provider;
    void provider().then((token) => {
      if (token) this.token = token;
      this.flushAuth(token);
    });
  }

  clearTokenProvider(): void {
    this.tokenProvider = null;
    this.flushAuth(null);
    this.resetAuthPromise();
  }

  isAuthenticated(): boolean {
    return !!this.token;
  }

  private gatewayBase(): string {
    const configured = import.meta.env.VITE_ALLTERNIT_GATEWAY_URL || 'https://api.allternit.com';
    return String(configured).replace(/\/+$/, '');
  }

  private async resolveToken(): Promise<string | null> {
    if (this.token) return this.token;
    if (this.tokenProvider) return this.tokenProvider();
    // The auth provider may not have mounted yet. Wait briefly for a token
    // or provider to be registered before sending an unauthenticated request.
    if (this.authPromise) {
      const timeoutMs = Number(import.meta.env.VITE_ALLTERNIT_AUTH_TIMEOUT_MS || 3000);
      const timeout = new Promise<string | null>((resolve) =>
        window.setTimeout(() => resolve(null), timeoutMs)
      );
      return Promise.race([this.authPromise, timeout]);
    }
    return null;
  }

  private async request<T>(
    method: string,
    path: string,
    body?: unknown,
    options: RequestInit = {}
  ): Promise<T> {
    const normalizedPath = path.startsWith('/') ? path : `/${path}`;
    const token = await this.resolveToken();
    const headers: Record<string, string> = {
      'Content-Type': 'application/json',
      Accept: 'application/json',
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
      ...(options.headers as Record<string, string> || {}),
    };

    const controller = new AbortController();
    const timeoutMs = Number(import.meta.env.VITE_ALLTERNIT_API_TIMEOUT_MS || 15000);
    const timeoutId = window.setTimeout(() => controller.abort(), timeoutMs);

    const response = await fetch(`${this.gatewayBase()}${normalizedPath}`, {
      ...options,
      method,
      headers,
      signal: controller.signal,
      ...(body && method !== 'GET' ? { body: JSON.stringify(body) } : {}),
    }).finally(() => window.clearTimeout(timeoutId));

    if (!response.ok) {
      const errorData = await response.json().catch(() => ({}));
      // Platform API errors nest as { error: { type, code, message, param } }.
      const nested = errorData.error && typeof errorData.error === 'object' ? errorData.error : null;
      throw new AllternitApiError(
        (nested ? nested.message : errorData.error) || errorData.message || `HTTP ${response.status}`,
        response.status,
        (nested ? nested.code : undefined) ?? errorData.code,
        errorData.details
      );
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
    const token = await this.resolveToken();
    const headers: Record<string, string> = {
      Accept: 'application/json',
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
      ...(options.headers as Record<string, string> || {}),
    };
    return fetch(`${this.gatewayBase()}${normalizedPath}`, {
      ...options,
      headers,
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
    const token = await this.resolveToken();
    const headers: Record<string, string> = {
      Accept: 'text/event-stream',
      ...(method === 'GET'
        ? {}
        : { 'Content-Type': 'application/json' }),
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
      ...(options.headers as Record<string, string> || {}),
    };

    const response = await fetch(`${this.gatewayBase()}${normalizedPath}`, {
      ...options,
      method,
      headers,
      body:
        method === 'GET' || body === undefined
          ? undefined
          : JSON.stringify(body),
      signal: options.signal ?? null,
    });

    if (!response.ok || !response.body) {
      const errorData = await response.json().catch(() => ({}));
      throw new AllternitApiError(
        errorData.error || errorData.message || `HTTP ${response.status}`,
        response.status,
        errorData.code,
        errorData.details
      );
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
