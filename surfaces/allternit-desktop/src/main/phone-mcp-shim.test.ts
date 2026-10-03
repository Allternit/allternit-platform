import { spawn } from 'node:child_process';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, describe, expect, it } from 'vitest';
import { startPhoneGateway } from './phone-gateway.js';
import { PhoneToolRunner, type AdbCall } from './phone-tools.js';

const shim = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../resources/phone/phone-mcp-shim.cjs');
const cleanups: Array<() => void> = [];
afterEach(() => cleanups.splice(0).forEach((c) => c()));

async function rpc(gatewayFile: string, messages: object[]): Promise<any[]> {
  const child = spawn(process.execPath, [shim], { env: { ...process.env, ALLTERNIT_PHONE_GATEWAY_FILE: gatewayFile } });
  cleanups.push(() => child.kill());
  let out = '';
  child.stdout.on('data', (d) => (out += d));
  for (const m of messages) child.stdin.write(JSON.stringify(m) + '\n');
  const want = messages.filter((m: any) => m.id !== undefined).length;
  for (let i = 0; i < 100 && out.split('\n').filter(Boolean).length < want; i += 1) await new Promise((r) => setTimeout(r, 50));
  return out.split('\n').filter(Boolean).map((l) => JSON.parse(l));
}

describe('phone MCP shim', () => {
  it('speaks MCP over stdio and forwards to the gateway', async () => {
    const adb = (async () => ({ stdout: '', stderr: '', code: 0 })) as unknown as AdbCall;
    adb.binary = async () => Buffer.from([0x89, 0x50, 0x4e, 0x47, 0, 0, 0, 0]);
    const runner = new PhoneToolRunner({
      adb,
      gate: { request: async () => 'denied' },
      rates: { load: () => [], save: () => {} },
      artemis: () => null,
      onlineSerials: () => ['a:1'],
    });
    const gw = await startPhoneGateway({ token: 't0ken', runner });
    cleanups.push(gw.close);
    const file = path.join(fs.mkdtempSync(path.join(os.tmpdir(), 'shim-')), 'gw.json');
    fs.writeFileSync(file, JSON.stringify({ url: gw.url, token: 't0ken' }));

    const replies = await rpc(file, [
      { jsonrpc: '2.0', id: 1, method: 'initialize', params: {} },
      { jsonrpc: '2.0', method: 'notifications/initialized' },
      { jsonrpc: '2.0', id: 2, method: 'tools/list' },
      { jsonrpc: '2.0', id: 3, method: 'tools/call', params: { name: 'phone.screenshot', arguments: {} } },
      { jsonrpc: '2.0', id: 4, method: 'tools/call', params: { name: 'phone.call.dial', arguments: { number: '+15550102030' } } },
    ]);
    const byId = (id: number) => replies.find((r) => r.id === id);
    expect(byId(1).result.serverInfo.name).toBe('allternit-phone');
    expect(byId(2).result.tools.map((t: any) => t.name)).toContain('phone.sms.send');
    expect(byId(3).result.content[0]).toMatchObject({ type: 'image', mimeType: 'image/png' });
    expect(byId(4).result.isError).toBe(true);
    expect(JSON.parse(byId(4).result.content[0].text).error).toBe('approval_denied');
  });

  it('says Desktop is not running when the gateway file is missing', async () => {
    const replies = await rpc('/nonexistent/gw.json', [{ jsonrpc: '2.0', id: 1, method: 'tools/list' }]);
    expect(replies[0].error.message).toMatch(/not running/);
  });
});
