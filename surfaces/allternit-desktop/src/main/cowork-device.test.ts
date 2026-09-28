import { describe, expect, it } from 'vitest';
import {
  coworkDeviceHeaders,
  coworkDeviceInfo,
  isInAppBrowsableUrl,
  platformLabel,
  shortHostname,
} from './cowork-device';

function memoryStore(initial?: string) {
  let id = initial;
  return {
    get: () => id,
    set: (_key: 'coworkDeviceId', value: string) => { id = value; },
  };
}

describe('cowork device identity', () => {
  it('generates the id once and keeps it', () => {
    const store = memoryStore();
    const first = coworkDeviceInfo(store);
    expect(first.id).toMatch(/^desktop-[0-9a-f-]{36}$/);
    expect(coworkDeviceInfo(store).id).toBe(first.id);
    expect(coworkDeviceInfo(memoryStore('desktop-existing-id')).id).toBe('desktop-existing-id');
  });

  it('labels platforms and trims host suffixes', () => {
    expect(platformLabel('darwin')).toBe('macOS');
    expect(platformLabel('win32')).toBe('Windows');
    expect(shortHostname('Joes-MacBook-Pro.local')).toBe('Joes-MacBook-Pro');
    expect(shortHostname('box.example.com')).toBe('box');
  });

  it('keeps header values Latin-1 safe', () => {
    const headers = coworkDeviceHeaders({ id: 'desktop-1234', name: 'Allternit Desktop (Zoë’s Mac)', platform: 'macOS', kind: 'desktop' });
    expect(headers['X-Allternit-Device-Name']).toBe('Allternit Desktop (Zo??s Mac)');
    expect(headers['X-Allternit-Device-Id']).toBe('desktop-1234');
  });

  it('only routes web links into the built-in browser', () => {
    expect(isInAppBrowsableUrl('https://example.com/a')).toBe(true);
    expect(isInAppBrowsableUrl('mailto:a@b.c')).toBe(false);
    expect(isInAppBrowsableUrl('not a url')).toBe(false);
  });
});
