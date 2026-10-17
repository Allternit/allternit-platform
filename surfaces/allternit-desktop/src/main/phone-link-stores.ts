import * as fs from 'node:fs';
import * as path from 'node:path';

/** The Phone Link record files under <userData>/phone (Android app, iPhone helper). */
export const LINKED_PHONE_FILES = ['phone-link-android.json', 'phone-link-ios.json'] as const;

/** True when Phone Link has saved at least one phone in either store. */
export function hasLinkedPhones(dir: string): boolean {
  for (const file of LINKED_PHONE_FILES) {
    try {
      const records = JSON.parse(fs.readFileSync(path.join(dir, file), 'utf8')) as unknown;
      if (records && typeof records === 'object' && Object.keys(records).length > 0) return true;
    } catch {
      // Missing or unreadable store: no phones there.
    }
  }
  return false;
}
