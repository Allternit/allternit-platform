import { randomUUID } from 'node:crypto';
import { fail, ok, type AaiResult } from '@allternit/agent-gateway';
import { PairedLocalTransport, isObject, type Json } from './paired-local.js';

/** DSH launch-token exchange and authority-bound cookie, never an LLM key. */
export class DshLocalTransport {
  readonly local: PairedLocalTransport;
  private cookies = new Map<string, { token: string; cookie: string }>();
  constructor(options: { baseUrl: string; token?: string; fetch?: typeof fetch }) {
    this.local = new PairedLocalTransport('DSH', options);
  }
  owner() { return this.local.owner(); }
  private async cookie(): Promise<AaiResult<string>> {
    const token = this.local.credential();
    if (!token || !/^[A-Za-z0-9_-]{43}$/.test(token)) return fail('AUTH_REQUIRED', 'Supply the DSH Web launch token from your own instance.');
    const prior = this.cookies.get(this.owner());
    if (prior?.token === token) return ok(prior.cookie);
    try {
      const response = await (this.local.options.fetch ?? fetch)(`${this.local.base}/?token=${encodeURIComponent(token)}`, { redirect: 'manual', signal: AbortSignal.timeout(15_000) });
      void response.body?.cancel();
      if (response.status === 401 || response.status === 403) return fail('AUTH_REVOKED', 'DSH refused the launch token. Obtain the current token from your instance.');
      if (response.status !== 303 || response.headers.get('location') !== './') return fail('ADAPTER_DRIFT', 'DSH did not confirm its documented token exchange.');
      const candidates = response.headers.getSetCookie().map(value => value.split(';', 1)[0]).filter(value => /^dsh-auth-[A-Za-z0-9_-]+=[A-Za-z0-9_.-]+$/.test(value));
      if (candidates.length !== 1) return fail('ADAPTER_DRIFT', 'DSH did not return exactly one signed session cookie.');
      this.cookies.set(this.owner(), { token, cookie: candidates[0] });
      return ok(candidates[0]);
    } catch { return fail('VENDOR_UNAVAILABLE', 'The local DSH authentication exchange could not complete.'); }
  }
  async json(path: string, body: Json): Promise<AaiResult<Json>> {
    if (!/^\/api\/[a-zA-Z0-9/_-]+$/.test(path)) return fail('POLICY_DENIED', 'This DSH route is not permitted.');
    const cookie = await this.cookie(); if (!cookie.ok) return cookie;
    try {
      const response = await (this.local.options.fetch ?? fetch)(this.local.base + path, { method: 'POST', redirect: 'error', signal: AbortSignal.timeout(15_000), headers: { cookie: cookie.value, 'content-type': 'application/json', accept: 'application/json' }, body: JSON.stringify(body) });
      if (!response.ok) {
        void response.body?.cancel();
        if ([401,403].includes(response.status)) { this.cookies.delete(this.owner()); return fail('AUTH_REVOKED', 'DSH refused this authenticated session. Reconnect with the current launch token.'); }
        return fail('VENDOR_UNAVAILABLE', `DSH returned HTTP ${response.status}. Do not retry mutations automatically.`, { retryable: false });
      }
      const value: unknown = await response.json();
      return isObject(value) ? ok(value) : fail('ADAPTER_DRIFT', 'DSH returned an invalid response.');
    } catch { return fail('VENDOR_UNAVAILABLE', 'DSH did not confirm this request. Its outcome may be uncertain.', { retryable: false }); }
  }
  async rpc(endpoint: string, args: Json): Promise<AaiResult<unknown>> {
    if (!/^session\/(prompt|page|projections)$/.test(endpoint)) return fail('POLICY_DENIED', 'This DSH session operation is not enabled.');
    const rpcId = randomUUID();
    const response = await this.json(`/api/${endpoint}`, { type: 'client-request', rpcId, method: endpoint, payload: { args } });
    if (!response.ok) return response;
    if (response.value.type !== 'server-response' || response.value.rpcId !== rpcId || !isObject(response.value.result)) return fail('ADAPTER_DRIFT', 'DSH returned a foreign RPC response.');
    const result = response.value.result;
    if (result.ok === true) return ok(result.value);
    return fail('POLICY_DENIED', 'DSH refused this session operation. Review the vendor conversation.', { retryable: false });
  }
}
