/**
 * The USB Agent firmware image Desktop ships (resources/phone/usb-agent,
 * built by hardware/usb-agent/build-firmware.sh from the public source in
 * hardware/usb-agent/firmware). The Connect an iPhone setup flashes it from
 * the window with esptool-js over Web Serial, so a user needs nothing but
 * the board and a USB cable.
 */

import { createHash } from 'node:crypto';
import * as fs from 'node:fs';
import * as path from 'node:path';

export const ESPRESSIF_VID = 0x303a;

export interface FirmwareManifest {
  version: string;
  chip: string;
  offset: number;
  size: number;
  sha256: string;
}

export interface BundledFirmware extends FirmwareManifest {
  data: Uint8Array;
}

/** Reads and verifies the bundled image; throws when it's missing or doesn't match its manifest. */
export function loadFirmware(dir: string): BundledFirmware {
  const manifest = JSON.parse(fs.readFileSync(path.join(dir, 'firmware.json'), 'utf8')) as FirmwareManifest;
  if (!/^\d+\.\d+\.\d+$/.test(manifest.version) || manifest.chip !== 'esp32s3' || manifest.offset !== 0) {
    throw new Error('The bundled USB Agent firmware manifest is invalid.');
  }
  const data = fs.readFileSync(path.join(dir, 'usb-agent-firmware.bin'));
  const sha = createHash('sha256').update(data).digest('hex');
  if (data.length !== manifest.size || sha !== manifest.sha256) {
    throw new Error('The bundled USB Agent firmware is damaged (checksum mismatch). Reinstall Allternit Desktop.');
  }
  return { ...manifest, data: new Uint8Array(data) };
}

/** -1, 0 or 1, comparing dotted versions numerically; unknown versions sort lowest. */
export function compareVersions(a: string | null, b: string | null): number {
  const parse = (v: string | null) => (v && /^\d+(\.\d+)*$/.test(v) ? v.split('.').map(Number) : null);
  const x = parse(a);
  const y = parse(b);
  if (!x && !y) return 0;
  if (!x) return -1;
  if (!y) return 1;
  for (let i = 0; i < Math.max(x.length, y.length); i += 1) {
    const d = (x[i] ?? 0) - (y[i] ?? 0);
    if (d) return Math.sign(d);
  }
  return 0;
}

/** Electron reports USB ids as hex strings ("303a"); be tolerant of decimal and 0x forms. */
export function usbId(value: string | number | undefined): number | null {
  if (value === undefined || value === '') return null;
  if (typeof value === 'number') return value;
  const v = value.trim().toLowerCase();
  if (/^0x[0-9a-f]+$/.test(v)) return parseInt(v.slice(2), 16);
  if (/^[0-9a-f]+$/.test(v)) return parseInt(v, 16);
  return null;
}

export interface SerialPortChoice {
  portId: string;
  vendorId?: string;
  productId?: string;
}

/** The serial port a Web Serial request gets: an Espressif board, or none. */
export function pickEspressifPort(ports: SerialPortChoice[]): string {
  return ports.find((p) => usbId(p.vendorId) === ESPRESSIF_VID)?.portId ?? '';
}
