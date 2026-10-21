import { describe, expect, it, vi } from 'vitest';
import { elementAt, parseUiLine, parseUiText, PhoneRunRecorder, redactArgs, RUN_IDLE_MS, type PhoneApi, type PhoneStepIn } from './phone-runs.js';

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
});
