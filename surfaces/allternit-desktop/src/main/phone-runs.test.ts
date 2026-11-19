import { describe, expect, it, vi } from 'vitest';
import { elementAt, httpPhoneApi, phoneMapsCall, PhoneApiUnavailable, parseUiLine, parseUiText, PhoneRunRecorder, redactArgs, RUN_IDLE_MS, type PhoneApi, type PhoneStepIn } from './phone-runs.js';

const UI = 'android.widget.FrameLayout {{0, 0}, {1080, 2400}}\nandroid.widget.Button "Dark theme" [com.android.settings:id/switch] {{100, 400}, {300, 80}}';

function recorder(now: () => number) {
  const posted: Array<{ serial: string; step: PhoneStepIn }> = [];
  const api = { postStep: vi.fn(async (serial: string, step: PhoneStepIn) => void posted.push({ serial, step })) } as unknown as PhoneApi;
  return { rec: new PhoneRunRecorder({ api: () => api, now }), posted };
}

const flush = () => new Promise((r) => setTimeout(r, 0));

describe('phone run history', () => {
  it('parses the shared read_ui text form', () => {
    expect(parseUiLine('android.widget.Button "OK" [a:id/ok] {{1, 2}, {3, 4}}')).toEqual({ className: 'android.widget.Button', label: 'OK', id: 'a:id/ok', rect: { x: 1, y: 2, w: 3, h: 4 } });
    expect(parseUiLine('nonsense')).toBeNull();
    expect(elementAt(parseUiText(UI), 150, 420)?.label).toBe('Dark theme');
  });

  it('never posts typed text, SMS bodies or numbers', () => {
    expect(redactArgs('phone.type', { text: 'hunter2', _run_id: 'r' })).toEqual({ text: { redacted: true, len: 7 } });
    expect(redactArgs('phone.sms.send', { to: '+1555', text: 'hi' })).toEqual({ to: { redacted: true, len: 5 }, text: { redacted: true, len: 2 } });
    expect(redactArgs('phone.type', { text: '{{query}}' })).toEqual({ text: '{{query}}' });
  });

  it('groups calls into runs and attaches the tapped element from the last read', async () => {
    let t = 1_000_000;
    const { rec, posted } = recorder(() => t);
    rec.record({ serial: 's', tool: 'phone.read_ui', args: {}, result: { ok: true, text: UI }, durationMs: 5 });
    rec.record({ serial: 's', tool: 'phone.tap', args: { x: 150, y: 420 }, result: { ok: true }, durationMs: 5 });
    await flush();
    expect(posted[1].step.target).toMatchObject({ label: 'Dark theme', class: 'android.widget.Button', screen: [1080, 2400] });
    expect(posted[0].step.run_id).toBe(posted[1].step.run_id);
    t += RUN_IDLE_MS + 1;
    rec.record({ serial: 's', tool: 'phone.home', args: {}, result: { ok: true }, durationMs: 1 });
    rec.record({ serial: 's', tool: 'phone.home', args: { _run_id: 'skill-1', _source: 'skill' }, result: { ok: true }, durationMs: 1 });
    await flush();
    expect(posted[2].step.run_id).not.toBe(posted[0].step.run_id);
    expect(posted[2].step.target).toBeUndefined();
    expect(posted[3].step).toMatchObject({ run_id: 'skill-1', source: 'skill' });
  });

  it('keeps the map marker from a go_to replay and no other underscored arg', async () => {
    const { rec, posted } = recorder(() => 1_000_000);
    rec.record({ serial: 's', tool: 'phone.tap', args: { x: 1, y: 2, _via: 'map', _map_screen: 'Wi-Fi', _pass: 'pass_1', _hold: 'h' }, result: { ok: true }, durationMs: 1 });
    rec.record({ serial: 's', tool: 'phone.tap', args: { x: 1, y: 2, _via: 'model', _map_screen: 'Wi-Fi' }, result: { ok: true }, durationMs: 1 });
    rec.record({ serial: 's', tool: 'phone.tap', args: { x: 1, y: 2 }, result: { ok: true }, durationMs: 1 });
    await flush();
    expect(posted[0].step).toMatchObject({ via: 'map', map_screen: 'Wi-Fi', args: { x: 1, y: 2 } });
    expect(JSON.stringify(posted[0].step.args)).not.toMatch(/_pass|_via|_hold|_map_screen/);
    expect(posted[1].step.via).toBeUndefined();
    expect(posted[1].step.map_screen).toBeUndefined();
    expect(posted[2].step.via).toBeUndefined();
  });
});

describe('app maps client', () => {
  const reply = (status: number, body: unknown) => vi.fn(async () => new Response(JSON.stringify(body), { status }));
  const client = (f: ReturnType<typeof reply>) => httpPhoneApi(() => 'http://127.0.0.1:1/', () => ({}), f as unknown as typeof fetch);

  it('calls the contract routes', async () => {
    const f = reply(200, { ok: true });
    const api = client(f);
    await api.maps('android');
    await api.map('com.a/b');
    await api.forgetMap('com.x');
    await api.explore('com.x', { serial: 's1', minutes: 5 });
    await api.stopRun('explore-ab');
    const calls = f.mock.calls as unknown as Array<[string, { method: string; body?: string }]>;
    expect(calls.map((c) => `${c[1].method} ${c[0]}`)).toEqual([
      'GET http://127.0.0.1:1/api/v1/phones/maps?platform=android',
      'GET http://127.0.0.1:1/api/v1/phones/maps/com.a%2Fb',
      'DELETE http://127.0.0.1:1/api/v1/phones/maps/com.x',
      'POST http://127.0.0.1:1/api/v1/phones/maps/com.x/explore',
      'POST http://127.0.0.1:1/api/aci/stop/explore-ab',
    ]);
    expect(JSON.parse(calls[3][1].body!)).toEqual({ serial: 's1', minutes: 5 });
  });

  it('passes contract error codes through, including 404 and 502', async () => {
    expect(await client(reply(403, { ok: false, error: 'money_app', message: 'no' })).explore('com.bank', { serial: 's' })).toMatchObject({ ok: false, error: 'money_app', status: 403 });
    expect(await client(reply(409, { ok: false, error: 'phone_busy', message: 'busy' })).explore('com.x', { serial: 's' })).toMatchObject({ error: 'phone_busy' });
    expect(await client(reply(404, { ok: false, error: 'map_not_found', message: 'none' })).map('com.x')).toMatchObject({ error: 'map_not_found' });
    expect(await client(reply(502, { ok: false, error: 'phone_unreachable', message: 'x' })).explore('com.x', { serial: 's' })).toMatchObject({ error: 'phone_unreachable' });
  });

  it('an old API (404 without a code) is unavailable', async () => {
    await expect(client(reply(404, {})).maps()).rejects.toBeInstanceOf(PhoneApiUnavailable);
  });

  it('phoneMapsCall validates, passes errors through and reports an old API as unsupported', async () => {
    const api = { explore: vi.fn(async () => ({ ok: true, run_id: 'explore-1', status: 'running' })), maps: vi.fn(async () => { throw new PhoneApiUnavailable('x'); }) } as unknown as PhoneApi;
    expect(await phoneMapsCall(api, 'explore', { app: 'com.x', serial: 's', minutes: 99 })).toMatchObject({ error: 'invalid_args' });
    expect(await phoneMapsCall(api, 'explore', { app: 'com.x' })).toMatchObject({ error: 'invalid_args' });
    expect(await phoneMapsCall(api, 'explore', { app: 'com.x', serial: 's', max_screens: 10 })).toMatchObject({ ok: true, run_id: 'explore-1' });
    expect(api.explore).toHaveBeenCalledWith('com.x', { serial: 's', minutes: undefined, max_screens: 10 });
    expect(await phoneMapsCall(api, 'list', {})).toMatchObject({ ok: false, error: 'unsupported' });
    expect(await phoneMapsCall(null, 'list', {})).toMatchObject({ error: 'unsupported' });
  });
});
