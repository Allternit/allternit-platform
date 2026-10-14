import { AsyncLocalStorage } from 'node:async_hooks';
import { fail, ok, type AaiResult } from '@allternit/agent-gateway';

export type Json = Record<string, unknown>;
export const isObject = (value: unknown): value is Json => !!value && typeof value === 'object' && !Array.isArray(value);
export const isId = (value: unknown): value is string => typeof value === 'string' && /^[\w-]+$/.test(value);
type Scope = { binding?: { id?: string; accountBindingId?: string }; credential?: { apiKey: string } };

export class PairedLocalTransport {
  readonly base: string;
  constructor(readonly vendor: string, readonly options: { baseUrl: string; token?: string; fetch?: typeof fetch }) {
    const url = new URL(options.baseUrl);
    if (!['127.0.0.1', '[::1]', 'localhost'].includes(url.hostname) || !['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.search || url.hash || url.pathname !== '/') throw new Error(`${vendor} endpoint must be a loopback origin`);
    this.base = url.origin;
  }
  private scope() { return (globalThis as unknown as Record<symbol, AsyncLocalStorage<Scope> | undefined>)[Symbol.for('allternit.aai.callScope')]?.getStore(); }
  credential() { return this.scope()?.credential?.apiKey ?? this.options.token; }
  owner() { return this.scope()?.binding?.accountBindingId ?? this.scope()?.binding?.id ?? 'local'; }
  async response(path: string, body?: Json, sse = false): Promise<AaiResult<Response>> {
    const token = this.credential();
    if (!token) return fail('AUTH_REQUIRED', `Supply the authorized ${this.vendor} host credential to connect.`);
    try {
      const result = await (this.options.fetch ?? fetch)(this.base + path, { method: body ? 'POST' : 'GET', redirect: 'error', signal: AbortSignal.timeout(15_000), headers: { authorization: `Bearer ${token}`, accept: sse ? 'text/event-stream' : 'application/json', ...(body ? { 'content-type': 'application/json' } : {}) }, ...(body ? { body: JSON.stringify(body) } : {}) });
      if (result.ok) return ok(result);
      void result.body?.cancel();
      if (result.status === 401 || result.status === 403) return fail('AUTH_REVOKED', `${this.vendor} refused this host credential or its permissions.`);
      if (result.status === 429) return fail('RATE_LIMITED', `${this.vendor} is rate limiting this connection.`, { retryable: !body });
      return fail('VENDOR_UNAVAILABLE', `${this.vendor} returned HTTP ${result.status}.`, { retryable: !body });
    } catch {
      return fail('VENDOR_UNAVAILABLE', body ? `${this.vendor} did not confirm this request. Its outcome may be uncertain; do not retry automatically.` : `${this.vendor} is unreachable.`, { retryable: !body });
    }
  }
  async json(path: string, body?: Json): Promise<AaiResult<Json>> {
    const result = await this.response(path, body);
    if (!result.ok) return result;
    try { const value: unknown = await result.value.json(); return isObject(value) ? ok(value) : fail('ADAPTER_DRIFT', `${this.vendor} returned an invalid response.`); }
    catch { return fail('ADAPTER_DRIFT', `${this.vendor} returned invalid JSON.`, { retryable: false }); }
  }
}
