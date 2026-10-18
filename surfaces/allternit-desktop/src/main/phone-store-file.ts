/**
 * The JSON files under <userData>/phone (phones.json and friends). No
 * Electron imports, so stores built on them are testable on disk.
 */

import * as fs from 'node:fs';
import * as path from 'node:path';
import type { PhoneStore, StoredPhone } from './phone-device-manager.js';

export function readJson<T>(file: string, fallback: T): T {
  try {
    return JSON.parse(fs.readFileSync(file, 'utf8')) as T;
  } catch {
    return fallback;
  }
}

export function writeJson(file: string, value: unknown, mode = 0o600): void {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, JSON.stringify(value, null, 2), { mode });
}

/** phones.json: the paired Android phones, each record saved whole (Call audio included). */
export function fileStore(file: string): PhoneStore {
  return { load: () => readJson<StoredPhone[]>(file, []), save: (phones) => writeJson(file, phones) };
}
