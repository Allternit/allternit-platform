import { describe, expect, it } from 'vitest';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { hasLinkedPhones } from './phone-link-stores.js';

const dir = () => fs.mkdtempSync(path.join(os.tmpdir(), 'pl-stores-'));

describe('hasLinkedPhones', () => {
  it('is false with no stores, empty stores or unreadable ones', () => {
    const d = dir();
    expect(hasLinkedPhones(d)).toBe(false);
    fs.writeFileSync(path.join(d, 'phone-link-android.json'), '{}');
    fs.writeFileSync(path.join(d, 'phone-link-ios.json'), 'not json');
    expect(hasLinkedPhones(d)).toBe(false);
  });
  it('is true when either store holds a phone', () => {
    const a = dir();
    fs.writeFileSync(path.join(a, 'phone-link-android.json'), JSON.stringify({ 'dev-1': { deviceId: 'dev-1' } }));
    expect(hasLinkedPhones(a)).toBe(true);
    const i = dir();
    fs.writeFileSync(path.join(i, 'phone-link-ios.json'), JSON.stringify({ udid: { udid: 'udid' } }));
    expect(hasLinkedPhones(i)).toBe(true);
  });
});
