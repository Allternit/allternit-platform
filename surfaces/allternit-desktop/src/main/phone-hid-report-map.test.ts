import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

/**
 * Guards a named regression risk (Phone Use gap, handoff 2026-11-17): the
 * Bluetooth HID report map is written twice, in Swift for the Mac helper and
 * the second iPhone/iPad (`bt-mac`, `bt-apple`: AllternitBTHID ReportMap.swift)
 * and in Kotlin for the Android app (`bt-android`: HidReports.kt). The Desktop
 * keyboard/mouse core (phone-hid-core.ts) sends the same reports through all
 * three, so the two maps must stay byte for byte the same. Neither language's
 * test runner sees the other file, so this test reads both sources.
 */

const SWIFT = fileURLToPath(new URL('../../../allternit-mobile/ios/Packages/AllternitBTHID/Sources/BTHIDCore/ReportMap.swift', import.meta.url));
const KOTLIN = fileURLToPath(new URL('../../../allternit-mobile/android/app/src/phoneshare/java/com/allternit/phonelink/core/HidReports.kt', import.meta.url));

/** The comma-separated items between `open` and the matching `close`, comments removed. */
function items(source: string, open: string, close: string): string[] {
  const start = source.indexOf(open);
  if (start < 0) throw new Error(`"${open}" not found`);
  const body = source.slice(start + open.length);
  const end = body.indexOf(close);
  return body
    .slice(0, end)
    .split('\n')
    .map((line) => line.replace(/\/\/.*$/, ''))
    .join('')
    .split(',')
    .map((t) => t.trim())
    .filter(Boolean);
}

/** A numeric constant declared in the source (`static let x: UInt8 = 0x65`, `const val X = 6`). */
function constant(source: string, name: string): number {
  const m = source.match(new RegExp(`(?:static let|const val)\\s+${name}\\s*(?::\\s*\\w+)?\\s*=\\s*(0x[0-9a-fA-F]+|\\d+)`));
  if (!m) throw new Error(`constant ${name} not found`);
  return Number(m[1]);
}

function resolve(tokens: string[], names: Record<string, number>): number[] {
  return tokens.map((t) => {
    const bare = t.replace(/^UInt8\((.*)\)$/, '$1');
    const n = /^(0x[0-9a-fA-F]+|\d+)$/.test(bare) ? Number(bare) : names[bare];
    if (n === undefined || !Number.isInteger(n) || n < 0 || n > 0xff) throw new Error(`cannot read "${t}" as a byte`);
    return n;
  });
}

describe('Bluetooth HID report map', () => {
  it('is byte for byte the same in Swift (Mac, second iPhone/iPad) and Kotlin (Android)', () => {
    const swift = readFileSync(SWIFT, 'utf8');
    const kotlin = readFileSync(KOTLIN, 'utf8');
    const swiftBytes = resolve(items(swift, 'public static let bytes: [UInt8] = [', '\n    ]'), {
      'HIDReportID.keyboard': constant(swift, 'keyboard'),
      'HIDReportID.mouse': constant(swift, 'mouse'),
      maxKeys: constant(swift, 'maxKeys'),
      maxKeyUsage: constant(swift, 'maxKeyUsage'),
    });
    const kotlinBytes = resolve(items(kotlin, 'val DESCRIPTOR: ByteArray = bytes(', '\n    )'), {
      KEYBOARD_ID: constant(kotlin, 'KEYBOARD_ID'),
      MOUSE_ID: constant(kotlin, 'MOUSE_ID'),
      MAX_KEYS: constant(kotlin, 'MAX_KEYS'),
      MAX_KEY_USAGE: constant(kotlin, 'MAX_KEY_USAGE'),
    });
    expect(swiftBytes.length).toBeGreaterThan(100);
    expect(kotlinBytes).toEqual(swiftBytes);
    // Both end the keyboard and mouse collections (a truncated read would still compare equal if both broke).
    expect(swiftBytes.at(-1)).toBe(0xc0);
  });
});
