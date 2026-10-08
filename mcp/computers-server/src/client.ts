/**
 * Thin HTTP client over the Allternit Computers REST API.
 *
 * First-party only: auth is a Clerk bearer token from `ALLTERNIT_TOKEN`,
 * sent as an `Authorization` header. No external developer API keys.
 *
 * @module client
 */

export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly body: string,
  ) {
    super(`API request failed (${status}): ${body}`);
    this.name = 'ApiError';
  }
}

export interface ComputersClientConfig {
  /** Base URL of allternit-api (default http://127.0.0.1:8013). */
  baseUrl: string;
  /** Clerk bearer token sent as `Authorization: Bearer …`. */
  token?: string;
}

export function configFromEnv(env: NodeJS.ProcessEnv = process.env): ComputersClientConfig {
  return {
    baseUrl: (env.ALLTERNIT_API_URL ?? 'http://127.0.0.1:8013').replace(/\/+$/, ''),
    // A Platform API project key may be given as ALLTERNIT_API_KEY or
    // ALLTERNIT_TOKEN; server.ts picks the /v1 client when it sees one.
    token: env.ALLTERNIT_API_KEY || env.ALLTERNIT_TOKEN || undefined,
  };
}

export class ComputersApiClient {
  constructor(private readonly config: ComputersClientConfig) {}

  private headers(extra?: Record<string, string>): Record<string, string> {
    const headers: Record<string, string> = { ...extra };
    if (this.config.token) {
      headers['authorization'] = `Bearer ${this.config.token}`;
    }
    return headers;
  }

  /** JSON request → parsed JSON body. Throws {@link ApiError} on non-2xx. */
  async requestJson<T = unknown>(
    method: string,
    path: string,
    body?: unknown,
    approvalId?: string,
  ): Promise<T> {
    const url = this.withApproval(path, approvalId);
    const response = await fetch(`${this.config.baseUrl}${url}`, {
      method,
      headers: this.headers(
        body === undefined ? undefined : { 'content-type': 'application/json' },
      ),
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const text = await response.text();
    if (!response.ok) {
      throw new ApiError(response.status, text);
    }
    return (text ? JSON.parse(text) : null) as T;
  }

  /** Raw string body (YAML/JSON template doc). Throws {@link ApiError}. */
  async requestRaw<T = unknown>(
    method: string,
    path: string,
    body: string,
    contentType: string,
    approvalId?: string,
  ): Promise<T> {
    const url = this.withApproval(path, approvalId);
    const response = await fetch(`${this.config.baseUrl}${url}`, {
      method,
      headers: this.headers({ 'content-type': contentType }),
      body,
    });
    const text = await response.text();
    if (!response.ok) {
      throw new ApiError(response.status, text);
    }
    return (text ? JSON.parse(text) : null) as T;
  }

  /** Raw-bytes upload (application/octet-stream). Throws {@link ApiError}. */
  async uploadBytes(path: string, bytes: Uint8Array, approvalId?: string): Promise<unknown> {
    const url = this.withApproval(path, approvalId);
    const response = await fetch(`${this.config.baseUrl}${url}`, {
      method: 'POST',
      headers: this.headers({ 'content-type': 'application/octet-stream' }),
      body: bytes as unknown as NonNullable<RequestInit['body']>,
    });
    const text = await response.text();
    if (!response.ok) {
      throw new ApiError(response.status, text);
    }
    return text ? JSON.parse(text) : null;
  }

  /** Binary download → bytes + declared content type. Throws {@link ApiError}. */
  async downloadBytes(path: string): Promise<{ bytes: Uint8Array; contentType: string }> {
    const response = await fetch(`${this.config.baseUrl}${path}`, {
      headers: this.headers(),
    });
    if (!response.ok) {
      const text = await response.text();
      throw new ApiError(response.status, text);
    }
    const bytes = new Uint8Array(await response.arrayBuffer());
    return { bytes, contentType: response.headers.get('content-type') ?? 'application/octet-stream' };
  }

  private withApproval(path: string, approvalId?: string): string {
    if (!approvalId) return path;
    const separator = path.includes('?') ? '&' : '?';
    return `${path}${separator}approval_id=${encodeURIComponent(approvalId)}`;
  }
}
