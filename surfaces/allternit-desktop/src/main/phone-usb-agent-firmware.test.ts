import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { compareVersions, loadFirmware, pickEspressifPort, usbId } from './phone-usb-agent-firmware.js';

const bundled = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../resources/phone/usb-agent');
const firmwareSource = path.resolve(bundled, '../../../../../hardware/usb-agent/firmware/usb_agent/usb_agent.ino');

describe('bundled USB Agent firmware', () => {
  it('ships an image that matches its manifest and the source version', () => {
    const fw = loadFirmware(bundled);
    expect(fw.chip).toBe('esp32s3');
    expect(fw.data.length).toBe(fw.size);
    // A firmware change without rebuilding the image (hardware/usb-agent/build-firmware.sh) fails here.
    const source = fs.readFileSync(firmwareSource, 'utf8');
    expect(source).toContain(`#define FIRMWARE_VERSION "${fw.version}"`);
    // ESP32 image magic at the bootloader offset (0x0 on the S3).
    expect(fw.data[0]).toBe(0xe9);
  });

  it('refuses a damaged image', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'fw-'));
    fs.copyFileSync(path.join(bundled, 'firmware.json'), path.join(dir, 'firmware.json'));
    const data = fs.readFileSync(path.join(bundled, 'usb-agent-firmware.bin'));
    data[1000] ^= 0xff;
    fs.writeFileSync(path.join(dir, 'usb-agent-firmware.bin'), data);
    expect(() => loadFirmware(dir)).toThrow(/damaged/);
  });
});

describe('firmware helpers', () => {
  it('compares versions numerically', () => {
    expect(compareVersions('1.0.0', '1.0.1')).toBe(-1);
    expect(compareVersions('1.10.0', '1.9.9')).toBe(1);
    expect(compareVersions('1.0', '1.0.0')).toBe(0);
    expect(compareVersions(null, '1.0.0')).toBe(-1);
  });

  it('reads USB ids and only ever picks an Espressif port', () => {
    expect(usbId('303a')).toBe(0x303a);
    expect(usbId('0x303A')).toBe(0x303a);
    expect(usbId(12346)).toBe(0x303a);
    expect(pickEspressifPort([{ portId: 'a', vendorId: '10c4' }, { portId: 'b', vendorId: '303a' }])).toBe('b');
    expect(pickEspressifPort([{ portId: 'a', vendorId: '10c4' }])).toBe('');
    expect(pickEspressifPort([])).toBe('');
  });
});
