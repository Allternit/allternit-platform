/**
 * Mocked-fetch helpers shared by the computers-server unit tests.
 *
 * Replaces `globalThis.fetch`, records every call (url, method, headers, body), and
 * routes the response through a caller-supplied handler.
 */

export interface RecordedCall {
  url: string;
  method: string;
  headers: Record<string, string>;
  body: string | null;
}

export function mockFetch(handler: (call: RecordedCall) => Response) {
  const calls: RecordedCall[] = [];
  globalThis.fetch = (async (input: unknown, init?: RequestInit) => {
    const url = typeof input === 'string' ? input : (input as Request).url;
    const headers: Record<string, string> = {};
    new Headers(init?.headers).forEach((value, key) => {
      headers[key] = value;
    });
    let body: string | null = null;
    if (typeof init?.body === 'string') {
      body = init.body;
    } else if (init?.body instanceof Blob) {
      body = Buffer.from(await init.body.arrayBuffer()).toString('utf8');
    } else if (init?.body instanceof Uint8Array) {
      body = Buffer.from(init.body).toString('utf8');
    }
    const call: RecordedCall = { url, method: init?.method ?? 'GET', headers, body };
    calls.push(call);
    return handler(call);
  }) as typeof fetch;
  return calls;
}

export const jsonResponse = (value: unknown, status = 200) =>
  new Response(JSON.stringify(value), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });

export const binaryResponse = (bytes: Uint8Array, contentType: string) =>
  new Response(bytes, { status: 200, headers: { 'Content-Type': contentType } });
