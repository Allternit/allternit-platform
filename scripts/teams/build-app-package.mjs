#!/usr/bin/env node
// Build the Microsoft Teams app package (manifest.zip) for Allternit's shared
// Teams app: one single-tenant Azure Bot + multi-tenant Entra app, one package
// installed per customer tenant.
//
//   node scripts/teams/build-app-package.mjs \
//     --app-id <Entra app id guid> [--version 1.0.0] [--app-url https://app.allternit.com] \
//     [--out dist/teams] [--rsc]
//
// Manifest schema 1.20 (https://learn.microsoft.com/en-us/microsoftteams/platform/resources/schema/manifest-schema).
// RSC (ChannelMessage.Read.Group / ChatMessage.Read.Chat) is behind --rsc and
// requires an admin consent flow in the customer tenant.
//
// Zero dependencies on purpose (the repo scripts run on bare node): PNG and
// ZIP are written by hand below. The generated icons are brand-color
// placeholders — replace color.png (192x192) and outline.png (32x32) with
// real brand assets before publishing to a tenant catalog.

import { deflateRawSync } from 'node:zlib';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';

const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, cur, i, all) => {
    if (cur.startsWith('--')) acc.push([cur.slice(2), all[i + 1] && !all[i + 1].startsWith('--') ? all[i + 1] : true]);
    return acc;
  }, []),
);

const appId = args['app-id'] || process.env.TEAMS_APP_ID || '';
if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(appId)) {
  fail('--app-id is required and must be the Entra application (client) id GUID ' +
    '(also the Azure Bot\'s Microsoft App Id — one app plays both roles).');
}
const version = typeof args.version === 'string' ? args.version : '1.0.0';
if (!/^\d+\.\d+\.\d+$/.test(version)) fail(`--version must look like 1.0.0, got "${version}"`);
const appUrl = (typeof args['app-url'] === 'string' ? args['app-url'] : 'https://app.allternit.com').replace(/\/+$/, '');
const outDir = resolve(typeof args.out === 'string' ? args.out : 'dist/teams');
const rsc = args.rsc === true;

const appHost = new URL(appUrl).host; // app.allternit.com
const manifestVersion = '1.20';
const schemaUrl = `https://developer.microsoft.com/json-schemas/teams/v${manifestVersion}/MicrosoftTeams.schema.json`;

const manifest = {
  $schema: schemaUrl,
  manifestVersion,
  version,
  id: appId,
  developer: {
    name: 'Allternit',
    websiteUrl: 'https://allternit.com',
    privacyUrl: 'https://allternit.com/privacy',
    termsOfUseUrl: 'https://allternit.com/terms',
  },
  name: { short: 'Allternit', full: 'Allternit — your AI team, in Teams' },
  description: {
    short: 'Talk to your Allternit bots in Teams.',
    full: 'Allternit bots answer in Teams chats and channels. Every conversation is a thread in Allternit; mention @Allternit to pick which bot replies. Install the app, sign in with Microsoft, and your Allternit runtime does the rest.',
  },
  icons: { color: 'color.png', outline: 'outline.png' },
  accentColor: '#0B0B0C',
  // The bot is the same Entra app that provides SSO and the Graph lanes.
  bots: [
    {
      botId: appId,
      scopes: ['personal', 'team', 'groupChat'],
      isNotificationOnly: false,
      supportsCalling: false,
      supportsVideo: false,
    },
  ],
  staticTabs: [
    {
      entityId: 'allternit.conversations',
      name: 'Conversations',
      contentUrl: `${appUrl}/teams/conversations`,
      websiteUrl: appUrl,
      scopes: ['personal'],
    },
    {
      entityId: 'allternit.home',
      name: 'Allternit',
      contentUrl: `${appUrl}/teams/tab`,
      websiteUrl: appUrl,
      scopes: ['personal'],
    },
  ],
  // Channel bindings: the settings page picks which Allternit bot a channel
  // or group chat talks to (written into channel_account_bots).
  configurableTabs: [
    {
      configurationUrl: `${appUrl}/teams/configure`,
      canUpdateConfiguration: true,
      scopes: ['team', 'groupChat'],
    },
  ],
  webApplicationInfo: {
    id: appId,
    resource: `api://${appHost}/${appId}`,
  },
  validDomains: [appHost],
};

if (rsc) {
  // Resource-specific consent lets the bot read messages in the channels and
  // chats it is installed in, without @mentions. Admin consent required.
  manifest.authorization = {
    permissions: {
      resourceSpecific: [
        { name: 'ChannelMessage.Read.Group', type: 'Application' },
        { name: 'ChatMessage.Read.Chat', type: 'Application' },
      ],
    },
  };
}

validate(manifest);

const colorPng = png(192, 192, [11, 11, 12, 255], [245, 158, 11, 255]);
const outlinePng = png(32, 32, [0, 0, 0, 0], [245, 158, 11, 255]);
const zipPath = join(outDir, 'manifest.zip');
mkdirSync(outDir, { recursive: true });
writeFileSync(zipPath, zip([
  { name: 'manifest.json', data: Buffer.from(JSON.stringify(manifest, null, 2), 'utf8') },
  { name: 'color.png', data: colorPng },
  { name: 'outline.png', data: outlinePng },
]));

console.log(`wrote ${zipPath}`);
console.log(`  manifest.json  manifestVersion ${manifestVersion}, app id ${appId}`);
console.log(`  color.png      192x192 placeholder (replace with the brand mark)`);
console.log(`  outline.png    32x32 placeholder`);
console.log(`  rsc            ${rsc ? 'ChannelMessage.Read.Group + ChatMessage.Read.Chat' : 'off (pass --rsc)'}`);
console.log('');
console.log('Next: an admin uploads the zip (Teams admin center or Graph appCatalogs/teamsApps),');
console.log('then each user connects Allternit (POST /api/v1/channels/teams/connect).');

// ------------------------------------------------------------ self-checks

function validate(m) {
  const problems = [];
  if (m.manifestVersion !== manifestVersion) problems.push(`manifestVersion must be ${manifestVersion}`);
  for (const tab of [...m.staticTabs, ...m.configurableTabs]) {
    for (const url of [tab.contentUrl, tab.configurationUrl, tab.websiteUrl]) {
      if (!url) continue;
      const host = new URL(url).host;
      if (!m.validDomains.includes(host)) problems.push(`validDomains misses ${host} (used by ${tab.name || tab.entityId})`);
    }
  }
  for (const scope of m.bots[0].scopes) {
    if (!['personal', 'team', 'groupChat'].includes(scope)) problems.push(`unknown bot scope ${scope}`);
  }
  if (m.configurableTabs.some((t) => t.scopes.includes('personal'))) {
    problems.push('configurableTabs cannot scope to personal');
  }
  if (problems.length) fail(problems.join('\n'));
}

function fail(message) {
  console.error(`build-app-package: ${message}`);
  process.exit(1);
}

// ---------------------------------------------------- minimal PNG writer

function crc32(buf) {
  crc32.table ||= Array.from({ length: 256 }, (_, n) => {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    return c >>> 0;
  });
  let c = 0xffffffff;
  for (const b of buf) c = crc32.table[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, 'ascii'), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}

// Solid-fill RGBA PNG with a 2px border in the accent color.
function png(width, height, fill, border) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // color type RGBA
  const raw = Buffer.alloc(height * (1 + width * 4));
  for (let y = 0; y < height; y++) {
    const row = y * (1 + width * 4);
    raw[row] = 0; // filter: none
    for (let x = 0; x < width; x++) {
      const onBorder = x < 2 || y < 2 || x >= width - 2 || y >= height - 2;
      const c = onBorder ? border : fill;
      raw.set(c, row + 1 + x * 4);
    }
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateRawSync(raw)),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

// ----------------------------------------------------- minimal ZIP writer

function zip(entries) {
  const localParts = [];
  const central = [];
  let offset = 0;
  const dos = { time: 0, date: 0x21 }; // 1980-01-01, fixed for reproducibility
  for (const e of entries) {
    const name = Buffer.from(e.name, 'utf8');
    const crc = crc32(e.data);
    const compressed = deflateRawSync(e.data);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4); // version needed
    local.writeUInt16LE(0x0800, 6); // UTF-8 names
    local.writeUInt16LE(8, 8); // deflate
    local.writeUInt16LE(dos.time, 10);
    local.writeUInt16LE(dos.date, 12);
    local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(compressed.length, 18);
    local.writeUInt32LE(e.data.length, 22);
    local.writeUInt16LE(name.length, 26);
    local.writeUInt16LE(0, 28);
    localParts.push(local, name, compressed);

    const cd = Buffer.alloc(46);
    cd.writeUInt32LE(0x02014b50, 0);
    cd.writeUInt16LE(20, 4);
    cd.writeUInt16LE(20, 6);
    cd.writeUInt16LE(0x0800, 8);
    cd.writeUInt16LE(8, 10);
    cd.writeUInt16LE(dos.time, 12);
    cd.writeUInt16LE(dos.date, 14);
    cd.writeUInt32LE(crc, 16);
    cd.writeUInt32LE(compressed.length, 20);
    cd.writeUInt32LE(e.data.length, 24);
    cd.writeUInt16LE(name.length, 28);
    cd.writeUInt32LE(offset, 42);
    central.push(Buffer.concat([cd, name]));
    offset += local.length + name.length + compressed.length;
  }
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(entries.length, 8);
  end.writeUInt16LE(entries.length, 10);
  const centralBuf = Buffer.concat(central);
  end.writeUInt32LE(centralBuf.length, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...localParts, centralBuf, end]);
}
