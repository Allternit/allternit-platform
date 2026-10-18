import { describe, expect, it } from 'vitest';
import { CHANNEL_SPECS, PhoneChannelReader, trimPictures } from './phone-channel-readers.js';
import { parseUiTree } from './phone-ui-tree.js';

// Discord screens as the helper's read_ui prints them (trimmed to what the spec reads).
const DRAWER_DMS = [
  "        Button, label: 'Direct Messages', {{0, 80}, {126, 105}}, clickable selected",
  "        Button, label: ' allternit', {{0, 208}, {126, 105}}, clickable",
  "        Button, label: 'Add a Server', {{0, 313}, {126, 105}}, clickable",
  "        Button, label: 'allternit, Online', {{49, 1439}, {545, 56}}, clickable",
  "          Button, label: '2 mentions, Sam (direct message)', {{142, 431}, {564, 87}}, clickable",
  "            TextView, label: 'Sam', {{233, 440}, {89, 35}}",
  "            TextView, label: '3h', {{653, 443}, {32, 28}}",
  "            TextView, label: 'Sam: see you at 5', {{233, 475}, {145, 36}}",
].join('\n');
const DRAWER_SERVER = [
  "        Button, label: 'Direct Messages', {{0, 80}, {126, 105}}, clickable",
  "        Button, label: ' allternit', {{0, 208}, {126, 105}}, clickable selected",
  "          Button, label: 'general (text channel)', {{142, 413}, {564, 63}}, clickable",
  "          Button, label: '1 mention, ships (text channel)', {{142, 478}, {564, 63}}, clickable",
].join('\n');
const CONVERSATION = [
  "  Button, label: 'Back', {{14, 94}, {70, 70}}, clickable",
  "  TextView, label: 'October 7, 2026', id: 'com.discord:id/middle_text', {{287, 841}, {147, 28}}",
  "  RelativeLayout, label: 'Sam, 10/07/2026 11:32 AM', {{0, 897}, {720, 200}}, clickable",
  "    TextView, label: 'Sam', id: 'com.discord:id/author_name', {{112, 901}, {91, 38}}, clickable",
  "    TextView, label: '10/07/2026 11:32 AM', id: 'com.discord:id/timestamp', {{217, 909}, {188, 28}}, clickable",
  "    TextView, label: 'see you at 5', {{112, 950}, {500, 38}}",
  "    TextView, label: 'bring the phone', {{112, 990}, {500, 38}}",
  "  RelativeLayout, label: 'allternit, 10/07/2026 11:40 AM', {{0, 1100}, {720, 120}}, clickable",
  "    TextView, label: 'on my way', {{112, 1150}, {500, 38}}",
  "  RelativeLayout, label: 'Sam, 7:26 AM', {{0, 1230}, {720, 70}}, clickable",
  "    TextView, label: 'ok', {{112, 1250}, {500, 38}}",
  "  Button, label: 'Send a wave', {{208, 1303}, {304, 84}}, clickable",
  "    TextView, label: 'Wave to Sam', {{292, 1327}, {192, 35}}",
  "  EditText, label: 'Message @Sam', id: 'com.discord:id/chat_input_edit_text', {{112, 1437}, {340, 56}}, clickable editable",
].join('\n');

describe('parseUiTree', () => {
  it('reads type, label, id, bounds and flags', () => {
    const [n] = parseUiTree("    EditText, label: 'Message @Sam', id: 'com.discord:id/chat_input_edit_text', {{112, 1437}, {340, 56}}, clickable editable");
    expect(n).toMatchObject({ type: 'EditText', label: 'Message @Sam', id: 'com.discord:id/chat_input_edit_text', rect: { x: 112, y: 1437, w: 340, h: 56 }, clickable: true, editable: true });
  });
});

describe('Discord reader', () => {
  it('reads a conversation: grouped lines per author, mine marked, buttons ignored', () => {
    expect(CHANNEL_SPECS.discord.messages(parseUiTree(CONVERSATION), 'allternit').map(({ parts: _p, ...m }) => m)).toEqual([
      { author: 'Sam', time: '10/07/2026 11:32 AM', text: 'see you at 5\nbring the phone', mine: false },
      { author: 'allternit', time: '10/07/2026 11:40 AM', text: 'on my way', mine: true },
      { author: 'Sam', time: '7:26 AM', text: 'ok', mine: false },
    ]);
  });

  it('shows a link card as a picture cut from the screenshot, text stays text', async () => {
    const screen = [
      "Window(application), {{0, 0}, {720, 1600}}",
      "  RelativeLayout, label: 'allternit, 10/02/2026 9:50 AM', {{0, 178}, {720, 1184}}, clickable",
      "    RecyclerView, id: 'com.discord:id/accessories_view', {{0, 178}, {720, 1180}}",
      "      TextView, label: 'look at this', {{112, 180}, {500, 38}}",
      "      FrameLayout, {{112, 250}, {587, 1108}}",
      "        TextView, label: 'LinkedIn', id: 'com.discord:id/provider', {{140, 264}, {531, 24}}, clickable",
      "        TextView, label: 'Parted ways.', id: 'com.discord:id/description', {{140, 354}, {531, 363}}, clickable",
      "        ImageView, id: 'com.discord:id/inline_media_image_preview', {{140, 731}, {473, 613}}",
      "  EditText, label: 'Message #news', id: 'com.discord:id/chat_input_edit_text', {{112, 1438}, {340, 56}}, clickable editable",
    ].join('\n');
    const crops: unknown[] = [];
    let opened = false;
    const shots = ['SHOT', 'SHOT2'];
    const reader = new PhoneChannelReader({
      cacheFile: '/dev/null/x.json',
      wait: async () => undefined,
      // The second screenshot differs: the card is animated.
      crop: (b64, rect, w) => { crops.push([b64, rect, w]); return b64 === 'SHOT' ? 'data:image/jpeg;base64,AAA' : 'data:image/jpeg;base64,BBB'; },
      call: async (name) => name === 'phone.tap' ? ((opened = true), { ok: true }) : name === 'phone.read_ui' ? { ok: true, text: opened ? screen : DRAWER_DMS } : name === 'phone.screenshot' ? { ok: true, dataBase64: shots.shift() ?? 'SHOT' } : { ok: true },
    });
    const res = await reader.messages('link:x', 'discord', { id: 'dm:Sam', title: 'Sam', group: 'Direct messages', preview: null, time: null, unread: 0 });
    expect(res.ok && res.data[0].parts).toEqual([
      { kind: 'text', text: 'look at this' },
      { kind: 'image', alt: 'LinkedIn\nParted ways.', src: 'data:image/jpeg;base64,AAA', width: 587, height: 1108, at: { x: 112, y: 250, w: 587, h: 1108 }, animated: true },
    ]);
    expect(crops).toEqual([['SHOT', { x: 112, y: 250, w: 587, h: 1108 }, 720], ['SHOT2', { x: 112, y: 250, w: 587, h: 1108 }, 720]]);
  });

  it('lists DMs and server channels, one operation at a time, and keeps a local copy', async () => {
    const screens = [DRAWER_DMS, DRAWER_DMS, DRAWER_DMS, DRAWER_SERVER, DRAWER_SERVER];
    const calls: string[] = [];
    const reader = new PhoneChannelReader({
      cacheFile: '/dev/null/never-written.json',
      wait: async () => undefined,
      call: async (name, args) => {
        calls.push(name);
        if (name === 'phone.read_ui') return { ok: true, text: screens.length > 1 ? screens.shift()! : screens[0] };
        return { ok: true, ...(args as object) };
      },
    });
    const res = await reader.conversations('link:x', 'discord');
    expect(res.ok && res.data).toEqual([
      { id: 'dm:Sam', title: 'Sam', group: 'Direct messages', time: '3h', preview: 'Sam: see you at 5', unread: 2 },
      { id: 'allternit/general', title: '#general', group: 'allternit', time: null, preview: null, unread: 0 },
      { id: 'allternit/ships', title: '#ships', group: 'allternit', time: null, preview: null, unread: 1 },
    ]);
    expect(calls[0]).toBe('phone.open_app');
    expect(reader.cached('discord').conversations).toHaveLength(3);
  });

  it('says plainly when an app has no reader yet', async () => {
    const reader = new PhoneChannelReader({ cacheFile: '/dev/null/x.json', call: async () => ({ ok: true }) });
    expect(await reader.conversations('link:x', 'snapchat')).toMatchObject({ ok: false, error: 'unsupported_app' });
  });
});

it('keeps pictures for the most recently read conversations within the size cap', () => {
  const pic = (src: string) => ({ author: 'a', time: null, text: '', mine: false, parts: [{ kind: 'image' as const, alt: 'card', src, width: 1, height: 1 }] });
  const messages = {
    old: { data: [pic('x'.repeat(60))], readAt: '2026-10-01T00:00:00Z' },
    new: { data: [pic('y'.repeat(60))], readAt: '2026-10-18T00:00:00Z' },
  };
  trimPictures(messages, 100);
  expect(messages.new.data[0].parts[0].src).toBeDefined();
  expect(messages.old.data[0].parts[0]).toEqual({ kind: 'image', alt: 'card', width: 1, height: 1 });
});
