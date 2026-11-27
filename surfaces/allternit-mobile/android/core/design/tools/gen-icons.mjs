// Generates the Android Phosphor icon set (core/design ph_*.xml) from the official
// @phosphor-icons/react defs, maps every iOS SF Symbol to Phosphor (ICONS.md), and
// replaces hand-drawn module glyphs with the mapped ph_ drawable.
// usage: node core/design/tools/gen-icons.mjs <phosphor-react-dir> <android-dir> <sf-list-file>
import fs from 'node:fs';
import path from 'node:path';

const [, , PH, ANDROID, SFLIST] = process.argv;
const DEFS = path.join(PH, 'dist/defs');
const OUT = path.join(ANDROID, 'core/design/src/main/res/drawable');

const pascal = (k) => k.split('-').map((s) => s[0].toUpperCase() + s.slice(1)).join('');
function paths(name, weight) {
  const file = path.join(DEFS, `${pascal(name)}.es.js`);
  if (!fs.existsSync(file)) return null;
  const src = fs.readFileSync(file, 'utf8');
  const start = src.indexOf(`"${weight}",`);
  if (start < 0) return null;
  const next = src.indexOf('\n  [\n', start + 5);
  const block = src.slice(start, next < 0 ? undefined : next);
  const ds = [...block.matchAll(/d: "([^"]+)"/g)].map((m) => m[1]);
  return ds.length ? ds : null;
}

// SF Symbol (iOS) -> Phosphor icon (kebab name; "-fill" suffix = Phosphor fill weight).
const SF = {
  'antenna.radiowaves.left.and.right': 'broadcast', 'app.window': 'app-window', 'apple.terminal': 'terminal-window',
  archivebox: 'archive', 'arrow.clockwise': 'arrow-clockwise', 'arrow.counterclockwise': 'arrow-counter-clockwise',
  'arrow.down': 'arrow-down', 'arrow.down.circle': 'arrow-circle-down', 'arrow.down.right.and.arrow.up.left': 'arrows-in',
  'arrow.left': 'arrow-left', 'arrow.left.arrow.right': 'arrows-left-right', 'arrow.right': 'arrow-right',
  'arrow.triangle.2.circlepath': 'arrows-clockwise', 'arrow.triangle.branch': 'git-branch', 'arrow.up': 'arrow-up',
  'arrow.up.circle.fill': 'arrow-circle-up-fill', 'arrow.up.forward.app': 'arrow-square-out',
  'arrow.up.left.and.arrow.down.right': 'arrows-out', 'arrow.up.right': 'arrow-up-right', 'arrow.up.right.square': 'arrow-square-out',
  'arrow.up.to.line': 'arrow-line-up', 'arrow.uturn.left': 'arrow-u-up-left', at: 'at', bolt: 'lightning', 'bolt.fill': 'lightning-fill',
  'bolt.slash': 'lightning-slash', 'bolt.trianglebadge.exclamationmark': 'lightning-a', brain: 'brain', 'bubble.left': 'chat-circle',
  'bubble.left.and.bubble.right': 'chats-circle', 'building.2': 'buildings', calendar: 'calendar-blank',
  'calendar.badge.checkmark': 'calendar-check', 'calendar.badge.plus': 'calendar-plus', camera: 'camera', 'chart.bar': 'chart-bar',
  'chart.bar.xaxis': 'chart-bar-horizontal', checklist: 'list-checks', checkmark: 'check', 'checkmark.circle': 'check-circle',
  'checkmark.circle.fill': 'check-circle-fill', 'checkmark.shield': 'shield-check', 'chevron.down': 'caret-down',
  'chevron.left': 'caret-left', 'chevron.right': 'caret-right', 'chevron.up': 'caret-up', 'chevron.up.chevron.down': 'caret-up-down',
  circle: 'circle', 'circle.dotted.circle': 'circle-dashed', 'circle.fill': 'circle-fill', clock: 'clock',
  'clock.arrow.2.circlepath': 'clock-clockwise', 'clock.arrow.circlepath': 'clock-counter-clockwise', cloud: 'cloud', cpu: 'cpu',
  creditcard: 'credit-card', crown: 'crown', 'crown.fill': 'crown-fill', 'cursorarrow.motionlines': 'cursor-click',
  desktopcomputer: 'desktop', 'desktopcomputer.trianglebadge.exclamationmark': 'desktop', display: 'monitor', doc: 'file',
  'doc.badge.plus': 'file-plus', 'doc.on.doc': 'copy', 'doc.richtext': 'file-text', 'doc.text': 'file-text',
  'doc.viewfinder': 'scan', dollarsign: 'currency-dollar', 'dot.radiowaves.left.and.right': 'broadcast', ellipsis: 'dots-three',
  envelope: 'envelope', 'exclamationmark.circle': 'warning-circle', 'exclamationmark.circle.fill': 'warning-circle-fill',
  'exclamationmark.triangle': 'warning', 'exclamationmark.triangle.fill': 'warning-fill', externaldrive: 'hard-drive',
  'externaldrive.badge.exclamationmark': 'hard-drive', eye: 'eye', 'figure.walk.motion': 'person-simple-walk', flag: 'flag',
  flame: 'flame', flask: 'flask', 'floppy.disk': 'floppy-disk', folder: 'folder', 'folder.badge.plus': 'folder-plus', gamecontroller: 'game-controller',
  'gamecontroller.fill': 'game-controller-fill', gearshape: 'gear', globe: 'globe', graduationcap: 'graduation-cap',
  'hand.raised': 'hand', 'hand.raised.fill': 'hand-fill', 'hand.tap': 'hand-tap', 'hard.drive': 'hard-drive', hourglass: 'hourglass',
  house: 'house', 'icloud.and.arrow.down': 'cloud-arrow-down', 'info.circle': 'info', iphone: 'device-mobile',
  'iphone.gen3': 'device-mobile', 'iphone.radiowaves.left.and.right': 'device-mobile-speaker', keyboard: 'keyboard',
  laptopcomputer: 'laptop', 'line.3.horizontal': 'list', 'line.3.horizontal.decrease': 'funnel-simple',
  'line.3.horizontal.decrease.circle': 'funnel', link: 'link', 'link.badge.plus': 'link-simple', 'list.bullet': 'list-bullets',
  'list.bullet.rectangle': 'list-dashes', lock: 'lock', magnifyingglass: 'magnifying-glass', 'lock.fill': 'lock-fill', map: 'map-trifold', message: 'chat-teardrop',
  'mic.fill': 'microphone-fill', 'minus.circle.fill': 'minus-circle-fill', 'moon.fill': 'moon-fill', network: 'network',
  paintbrush: 'paint-brush', paperclip: 'paperclip', paperplane: 'paper-plane-tilt', 'pause.circle': 'pause-circle',
  'pause.fill': 'pause-fill', pencil: 'pencil-simple', 'pencil.circle.fill': 'pencil-circle', 'person.badge.plus': 'user-plus',
  'person.circle': 'user-circle', 'person.crop.circle': 'user-circle', 'person.crop.circle.badge.xmark': 'user-circle-minus',
  phone: 'phone', photo: 'image', 'photo.badge.exclamationmark': 'image-broken', 'photo.on.rectangle': 'images', 'pin.slash': 'push-pin-slash',
  play: 'play', 'play.fill': 'play-fill', plus: 'plus', 'plus.circle': 'plus-circle', 'plus.circle.fill': 'plus-circle-fill',
  'plus.forwardslash.minus': 'plus-minus', 'point.3.connected.trianglepath.dotted': 'graph',
  'point.topleft.down.to.point.bottomright.curvepath': 'path', powerplug: 'plug', 'puzzlepiece.extension': 'puzzle-piece',
  'puzzlepiece.extension.fill': 'puzzle-piece-fill', 'puzzlepiece.fill': 'puzzle-piece-fill', 'qrcode.viewfinder': 'qr-code',
  'rectangle.portrait.and.arrow.right': 'sign-out', 'rectangle.split.3x1': 'columns', repeat: 'repeat', return: 'arrow-elbow-down-left',
  safari: 'compass', signature: 'signature', 'slider.horizontal.3': 'sliders-horizontal', sparkle: 'sparkle', sparkles: 'sparkle',
  'sparkles.bubble.left.fill': 'chat-circle-dots-fill', square: 'square', 'square.and.arrow.down': 'download-simple',
  'square.and.arrow.up': 'export', 'square.and.pencil': 'note-pencil', 'square.fill': 'square-fill', 'square.grid.2x2': 'squares-four',
  star: 'star', 'star.fill': 'star-fill', 'stop.circle': 'stop-circle', 'stop.fill': 'stop-fill', storefront: 'storefront',
  'switch.2': 'toggle-left', tablecells: 'table', target: 'target', terminal: 'terminal', thermometer: 'thermometer', trash: 'trash',
  tray: 'tray', video: 'video-camera', 'video.badge.checkmark': 'video-camera', 'video.slash': 'video-camera-slash', wallet: 'wallet',
  waveform: 'waveform', 'waveform.path.ecg': 'heartbeat', 'wifi.exclamationmark': 'wifi-x', 'wifi.slash': 'wifi-slash', wrench: 'wrench',
  'wrench.and.screwdriver': 'wrench', xmark: 'x', 'xmark.circle': 'x-circle', 'xmark.circle.fill': 'x-circle-fill',
  // Composer / Code extras used by the ported screens.
  'diamond.fill': 'diamond-fill', 'square.stack.3d.up': 'stack', 'mic': 'microphone', 'waveform.circle.fill': 'waveform',
  'square.split.diagonal.2x2': 'git-fork', 'chevron.down.small': 'caret-down',
};

// Hand-drawn module glyphs -> the Phosphor icon for the same concept.
const GLYPH = {
  shell_x: 'x', shell_dots: 'dots-three', shell_clock: 'clock', shell_plus: 'plus', shell_note_pencil: 'note-pencil',
  shell_folder: 'folder', shell_bolt: 'lightning', shell_tray: 'tray', shell_list: 'list', shell_link: 'link',
  shell_file_text: 'file-text', shell_device_mobile: 'device-mobile', shell_desktop: 'desktop', shell_chat: 'chat-circle',
  shell_wifi_slash: 'wifi-slash', shell_star_fill: 'star-fill', shell_star: 'star', shell_stack: 'stack', shell_shield_check: 'shield-check',
  shell_photo: 'image', shell_grid: 'squares-four', shell_grad_cap: 'graduation-cap', shell_caret_up: 'caret-up',
  shell_caret_right: 'caret-right', shell_waveform: 'waveform', shell_scribble: 'scribble-loop', shell_mic: 'microphone',
  shell_house: 'house', shell_file_plus: 'file-plus', shell_checklist: 'list-checks', shell_caret_down: 'caret-down',
  shell_camera: 'camera', shell_brain: 'brain', shell_bell_slash: 'bell-slash',
  bots_desktop: 'desktop', bots_chat: 'chat-circle', bots_caret_right: 'caret-right', bots_x: 'x', bots_tray: 'tray',
  bots_pencil: 'pencil-simple', bots_hand: 'hand', bots_grid: 'squares-four', bots_file_text: 'file-text', bots_list: 'list',
  bots_gear: 'gear', bots_eye: 'eye', bots_caret_down: 'caret-down', bots_stop: 'stop-fill', bots_star: 'star', bots_question: 'question',
  bots_plus: 'plus', bots_link: 'link', bots_house: 'house', bots_dots: 'dots-three', bots_clock: 'clock', bots_star_fill: 'star-fill',
  bots_photo: 'image', bots_folder: 'folder', bots_file_plus: 'file-plus', bots_brain: 'brain',
  chat_share: 'export', chat_retry: 'arrow-clockwise', chat_file: 'file', chat_caret_up: 'caret-up', chat_caret_down: 'caret-down',
  chat_pencil: 'pencil-simple', chat_gear: 'gear', chat_thumbs_up: 'thumbs-up', chat_thumbs_down: 'thumbs-down', chat_stop: 'stop-fill',
  chat_speaker: 'speaker-high', chat_question: 'question', chat_plugs: 'plugs', chat_hand: 'hand', chat_brain: 'brain',
  vc_bell: 'bell', vc_call_end: 'phone-x', vc_camera: 'camera', vc_chat: 'chat-circle', vc_clock: 'clock', vc_expand: 'arrows-out',
  vc_gear: 'gear', vc_mic_off: 'microphone-slash', vc_mic: 'microphone', vc_minimize: 'arrows-in', vc_photo: 'image',
  vc_speaker_off: 'speaker-slash', vc_speaker: 'speaker-high', vc_thumb_down: 'thumbs-down', vc_thumb_up: 'thumbs-up',
  vc_video: 'video-camera', vc_waveform: 'waveform',
  rfb_x: 'x', rfb_keyboard: 'keyboard', rfb_hand: 'hand', rfb_eye: 'eye', rfb_corners_out: 'corners-out',
};

// The existing ph_ set (converted from iOS PhIcons) is regenerated from the same source.
const EXISTING = fs.readdirSync(OUT).filter((f) => /^ph_.*\.xml$/.test(f)).map((f) => f.slice(3, -4).replace(/_/g, '-'));
const res = (k) => 'ph_' + k.replace(/-/g, '_');
function weightOf(k) {
  if (k.endsWith('-fill')) return [k.slice(0, -5), 'fill'];
  if (k.endsWith('-bold')) return [k.slice(0, -5), 'bold'];
  if (k.endsWith('-thin')) return [k.slice(0, -5), 'thin'];
  return [k, 'regular'];
}
const wanted = new Set([...Object.values(SF), ...Object.values(GLYPH), ...EXISTING]);
const missing = [];
for (const k of wanted) {
  const [base, weight] = weightOf(k);
  const ds = paths(base, weight);
  if (!ds) { missing.push(k); continue; }
  const body = ds.map((d) => `    <path\n        android:fillColor="#FF000000"\n        android:pathData="${d}" />`).join('\n');
  fs.writeFileSync(path.join(OUT, res(k) + '.xml'),
    `<?xml version="1.0" encoding="utf-8"?>\n<!-- Phosphor ${base} (${weight}), MIT, @phosphor-icons/react 2.1.10. Generated by core/design ICONS.md; do not edit. -->\n<vector xmlns:android="http://schemas.android.com/apk/res/android"\n    android:width="24dp"\n    android:height="24dp"\n    android:viewportWidth="256"\n    android:viewportHeight="256">\n${body}\n</vector>\n`);
}
if (missing.length) { console.error('MISSING phosphor:', missing.join(' ')); process.exit(1); }

// Replace hand-drawn references in Kotlin and XML.
const walk = (d, acc = []) => { for (const e of fs.readdirSync(d, { withFileTypes: true })) {
  if (e.name === 'build' || e.name.startsWith('.')) continue;
  const p = path.join(d, e.name);
  if (e.isDirectory()) walk(p, acc); else if (/\.(kt|xml)$/.test(e.name)) acc.push(p);
} return acc; };
let edits = 0;
for (const f of walk(ANDROID)) {
  const src = fs.readFileSync(f, 'utf8');
  let out = src.replace(/\b(?:DesignR|R)\.drawable\.((?:shell|bots|chat|vc|rfb)_[a-z0-9_]+)\b/g, (m, name) => {
    if (!GLYPH[name]) throw new Error(`unmapped ${name} in ${f}`);
    return `com.allternit.core.design.R.drawable.${res(GLYPH[name])}`;
  });
  out = out.replace(/@drawable\/((?:shell|bots|chat|vc|rfb)_[a-z0-9_]+)\b/g, (m, name) => `@drawable/${res(GLYPH[name])}`);
  if (out !== src) { fs.writeFileSync(f, out); edits++; }
}
// Delete the hand-drawn drawables.
let removed = 0;
for (const f of walk(ANDROID)) {
  const b = path.basename(f, '.xml');
  if (f.includes('/res/drawable') && GLYPH[b]) { fs.rmSync(f); removed++; }
}

// ICONS.md
const sfUsed = fs.readFileSync(SFLIST, 'utf8').split('\n').filter(Boolean);
const rows = sfUsed.map((s) => `| \`${s}\` | \`${SF[s] ?? '?'}\` | \`${SF[s] ? res(SF[s]) : '?'}\` |`);
const unmapped = sfUsed.filter((s) => !SF[s]);
const glyphRows = Object.entries(GLYPH).map(([g, k]) => `| \`${g}\` | \`${res(k)}\` |`);
fs.writeFileSync(path.join(ANDROID, 'core/design/ICONS.md'), `# Android icon set (Phosphor, mapped from iOS)

iOS draws its icons with SF Symbols, which Apple's licence keeps off Android. Desktop
(allternit-ai) uses \`@phosphor-icons/react\` and iOS ships the same family in
\`Assets.xcassets/PhIcons\`. Android uses one set: **Phosphor, regular weight** (fill
where iOS uses a \`.fill\` symbol), MIT licensed, generated from the official
\`@phosphor-icons/react@2.1.10\` path data into \`core/design/src/main/res/drawable/ph_*.xml\`.

Use \`com.allternit.core.design.R.drawable.ph_<name>\` everywhere. A feature module never
draws its own glyph: the same concept uses the same icon on every screen.

## Regenerate

\`\`\`sh
node core/design/tools/gen-icons.mjs <allternit-ai>/node_modules/@phosphor-icons/react surfaces/allternit-mobile/android sf.txt
\`\`\`

\`sf.txt\` is \`grep -rhoE '(systemName|systemImage): *"[^"]+"' ios/Features ios/Shared\`, de-duplicated.
The generator is \`core/design/tools/gen-icons.mjs\`.

## SF Symbol to Phosphor (${sfUsed.length} symbols used in ios/Features)

| SF Symbol (iOS) | Phosphor | Android drawable |
|---|---|---|
${rows.join('\n')}

## Retired hand-drawn glyphs

These per-module glyphs were drawn by hand in earlier waves. They are deleted and every
reference now points at the Phosphor drawable.

| Old drawable | Phosphor drawable |
|---|---|
${glyphRows.join('\n')}
`);
console.log(JSON.stringify({ generated: wanted.size, kotlinXmlFilesEdited: edits, removedDrawables: removed, unmappedSF: unmapped }));
