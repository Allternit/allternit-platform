import { describe, expect, it } from 'vitest';
import { KernelAdapter, KERNEL_SOURCE, toKernelItem } from './kernel-adapter.js';

describe('KernelAdapter', () => {
  it('maps a memory to a canonical item', () => {
    expect(toKernelItem({ id: 'm1', summary: 'Prefers tea', content: 'Said they prefer tea over coffee' })).toEqual({
      external_id: 'm1',
      text: 'Prefers tea\n\nSaid they prefer tea over coffee',
      memory_type: 'fact',
    });
  });

  it('posts upserts and deletes and never throws', async () => {
    const calls: { url: string; body: any }[] = [];
    const ok = (async (url: string, init: RequestInit) => {
      calls.push({ url, body: JSON.parse(String(init.body)) });
      return new Response('{}', { status: 200 });
    }) as unknown as typeof fetch;
    const a = new KernelAdapter('http://api', 't', ok);
    expect(await a.upsert([{ id: 'm1', summary: 's', content: 'c' }])).toBe(true);
    expect(await a.remove(['m1'])).toBe(true);
    expect(calls[0]).toMatchObject({ url: 'http://api/api/v1/memory/adapters/upsert', body: { source: KERNEL_SOURCE } });
    expect(calls[1]).toMatchObject({ url: 'http://api/api/v1/memory/adapters/delete', body: { external_ids: ['m1'] } });
    const down = (async () => {
      throw new Error('ECONNREFUSED');
    }) as unknown as typeof fetch;
    expect(await new KernelAdapter('http://api', 't', down).upsert([{ id: 'm', summary: '', content: 'c' }])).toBe(false);
  });
});
