// Files drive protocol v2 stub: an in-memory server for UI lanes. Node 20, no dependencies.
// Every JSON response body is validated against the contract schemas before it is sent
// (contracts/files-drive/schema-lib.mjs, the same validator generate.mjs uses).
import http from 'node:http';
import crypto from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { blake3hex } from './blake3.mjs';
import { validateType, defs } from '../../contracts/files-drive/schema-lib.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const FIX = path.resolve(here, '../../contracts/files-drive/fixtures');
const fx = (n) => JSON.parse(fs.readFileSync(path.join(FIX, `${n}.json`), 'utf8')).message;
const clone = (o) => JSON.parse(JSON.stringify(o));

export const SCENARIOS = ['awake', 'asleep', 'waking', 'conflict', 'slow', 'guest', 'error'];
const MAX_CHUNK = 4194304;
const MiB = 1048576;
const OWNER = { kind: 'owner', id: 'user_2abc', display_name: 'Eoj' };
const STATUS = {
  invalid_request: 400, invalid_name: 400, manifest_invalid: 400, chunk_hash_mismatch: 400,
  unauthorized: 401, passcode_required: 401, passcode_invalid: 401,
  forbidden: 403, feature_disabled: 403, scope_violation: 403, protected_root_locked: 403, agent_process_denied: 403, execute_denied: 403, guest_wake_not_allowed: 403, wake_denied: 403,
  not_found: 404, root_not_found: 404, upload_session_not_found: 404,
  already_exists: 409, rev_mismatch: 409, wake_consent_required: 409, chunk_missing: 409,
  share_expired: 410, share_revoked: 410, upload_session_expired: 410, cursor_expired: 410,
  payload_too_large: 413, integrity_failed: 422, rate_limited: 429,
  asleep: 503, waking: 503, offline: 503, replica_unavailable: 503, key_unavailable: 503,
  quota_exceeded: 507, unsupported: 501, internal: 500,
};
export const ERROR_STATUS = STATUS;
const MESSAGES = {
  wake_consent_required: 'This computer is asleep. Wake it to open the file?', wake_denied: 'Waking from Files is turned off for this computer.',
  waking: 'The computer is starting.', asleep: 'This computer is asleep and has no snapshot yet.', offline: 'This device is not reachable.',
  guest_wake_not_allowed: "The owner's computer is asleep. Try again later.", rev_mismatch: 'The item changed since you last saw it.',
  chunk_missing: 'Some chunks have not been uploaded.', cursor_expired: 'That cursor is too old. List from scratch.',
  feature_disabled: 'Files drive is not turned on for this account.', payload_too_large: 'The request body is too large.',
};

class StubError extends Error {
  constructor(code, message, extra = {}) { super(message || MESSAGES[code] || code); this.code = code; this.extra = extra; }
}
const b64u = (b) => Buffer.from(b).toString('base64url');
const nowIso = (ms = Date.now()) => new Date(ms).toISOString().replace(/\.\d+Z$/, 'Z');
const rid = (n = 10) => crypto.randomBytes(8).toString('hex').slice(0, n);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const mimeOf = (name) => ({ pdf: 'application/pdf', txt: 'text/plain', md: 'text/markdown', json: 'application/json', csv: 'text/csv', jpg: 'image/jpeg', jpeg: 'image/jpeg', png: 'image/png', mp3: 'audio/mpeg', mp4: 'video/mp4' })[name.split('.').pop().toLowerCase()] || 'application/octet-stream';
const splitExt = (name) => { const i = name.lastIndexOf('.'); return i > 0 ? [name.slice(0, i), name.slice(i)] : [name, '']; };
const seqOf = (cur) => parseInt(cur.slice(cur.lastIndexOf('.') + 1), 10);

export function createStub(opts = {}) {
  const cfg = {
    port: opts.port ?? 47820,
    computerId: opts.computerId || process.env.FILES_STUB_COMPUTER || 'cmp_cloud_4gb',
    scenario: opts.scenario || process.env.FILES_STUB_SCENARIO || 'awake',
    wakeMs: opts.wakeMs ?? Number(process.env.FILES_STUB_WAKE_MS || 3000),
    slowBps: opts.slowBps ?? Number(process.env.FILES_STUB_SLOW_BPS || 262144),
    heartbeatMs: opts.heartbeatMs ?? Number(process.env.FILES_STUB_HEARTBEAT_MS || 20000),
    uploadTtlS: opts.uploadTtlS ?? 3600,
    quiet: opts.quiet ?? false,
  };
  if (!SCENARIOS.includes(cfg.scenario)) throw new Error(`unknown scenario ${cfg.scenario}; use one of ${SCENARIOS.join(', ')}`);

  // ------------------------------------------------------------ token keys (Ed25519, like cloud-api)
  const { privateKey } = crypto.generateKeyPairSync('ed25519');
  const KID = 'stubkey1';
  const pubJwk = crypto.createPublicKey(privateKey).export({ format: 'jwk' });
  const PREFIX = { wake: 'awt1', files_guest: 'fgt1' }; // the schemas pin these prefixes (SPEC 8 said fd1)
  const signToken = (claims) => { const pre = PREFIX[claims.typ]; const p = b64u(JSON.stringify(claims)); return `${pre}.${p}.${b64u(crypto.sign(null, Buffer.from(`${pre}.${p}`), privateKey))}`; };
  const verifyToken = (tok, typ) => {
    const m = new RegExp(`^${PREFIX[typ]}\\.([A-Za-z0-9_-]+)\\.([A-Za-z0-9_-]+)$`).exec(tok || '');
    if (!m) throw new StubError('unauthorized', 'Bad capability token.');
    if (!crypto.verify(null, Buffer.from(`${PREFIX[typ]}.${m[1]}`), crypto.createPublicKey(privateKey), Buffer.from(m[2], 'base64url'))) throw new StubError('unauthorized', 'Bad token signature.');
    const c = JSON.parse(Buffer.from(m[1], 'base64url').toString());
    if (c.typ !== typ) throw new StubError('unauthorized', 'Wrong token type.');
    if (c.exp * 1000 < Date.now()) throw new StubError('unauthorized', 'Token expired.');
    return c;
  };

  // ------------------------------------------------------------ store
  let S;
  const sockets = new Set();
  const waiters = new Set();
  const wsClients = new Set();
  let wakeTimer = null;

  function seed() {
    S = {
      scenario: cfg.scenario, wakeSetting: 'ask', touches: 0, errCycle: 0,
      items: new Map(), roots: new Map(), chunks: new Map(), uploads: new Map(), trash: new Map(), versions: new Map(),
      comments: new Map(), logs: new Map(), shares: new Map(), links: new Map(), guests: new Map(), audit: [], events: [], jobs: new Map(),
      replicas: clone(fx('replicas').replicas), envelopes: new Map(), drops: [], snap: null, vSeq: 1,
    };
    for (const r of clone(fx('roots-awake').roots)) S.roots.set(r.id, r);
    S.roots.set('rt_shared', { id: 'rt_shared', kind: 'shared', name: 'Shared', path_hint: '~/Allternit/Shared', policy: { ...clone(S.roots.get('rt_inbox').policy), agent_process: false }, root_item_id: 'it_sharedrt0', parent_root_id: 'rt_home', cursor: 'c1.rt_shared.000000', provider_ready: true });
    for (const r of S.roots.values()) S.logs.set(r.id, { base: seqOf(r.cursor), seq: seqOf(r.cursor), entries: [] });
    const home = fx('list-home');
    const put = (item, content) => {
      const it = clone(item);
      const rec = { item: it, content: null, chunks: [], state: 'live' };
      S.items.set(it.id, rec);
      if (it.kind === 'file') setContent(rec, content ?? synth(it.name, it.size), true);
      else it.rev = `${it.meta_version}-dir`;
      return rec;
    };
    put(home.parent);
    for (const i of home.items) put(i);
    for (const r of S.roots.values()) if (!S.items.has(r.root_item_id)) put(folder(r.root_item_id, r.id, undefined, r.name));
    const mk = (id, root, parent, name, size, text) => put({ ...base(id, root, parent, name, 'file', size), mime: mimeOf(name) }, text ? Buffer.from(text) : undefined);
    mk('it_notes0001', 'rt_home', 'it_home0000', 'Notes.txt', 0, 'Stub notes: lease renewal is due 2027-01-15. Call the landlord about parking.\n');
    mk('it_scan00001', 'rt_inbox', 'it_inbox000', 'Scan-0001.pdf', 120 * 1024);
    mk('it_prof00001', 'rt_memory', 'it_mem00000', 'profile.md', 0, '# Profile\nStub memory file.\n');
    put(folder('it_shr000001', 'rt_shared', 'it_sharedrt0', 'Closing docs'));
    for (const i of fx('guest-list-view').items) put(i);
    mk('it_terms0001', 'rt_shared', 'it_shr000001', 'Terms.txt', 0, 'Closing terms (stub). Review before signing.\n');
    for (const rec of S.items.values()) if (rec.item.kind === 'file') rec.item.meta_version = Math.max(1, rec.item.meta_version);
    // shares + link tokens from the fixture, plus an expired and a revoked one
    for (const s of clone(fx('shares-list').shares)) {
      s.computer_id = cfg.computerId;
      const tok = s.link_url.split('/').pop();
      S.shares.set(s.share_id, s); if (!S.links.has(tok)) S.links.set(tok, { share_id: s.share_id, passcode: s.passcode_required ? '4417' : null }); // sh_old reuses a link url in the fixture
    }
    const extra = (id, tok, patch) => { const b = clone(S.shares.get('sh_view_download')); Object.assign(b, { share_id: id, link_url: `https://ai.allternit.com/s/${tok}`, passcode_required: false, guest_count: 0 }, patch); S.shares.set(id, b); S.links.set(tok, { share_id: id, passcode: null }); };
    extra('sh_expired', 'expired0stub', { expires_at: '2026-01-01T00:00:00Z' });
    extra('sh_revoked', 'revoked0stub', { revoked_at: '2026-11-14T00:00:00Z' });
    extra('sh_open', 'view_downloadopen', { expires_at: undefined });
    delete S.shares.get('sh_open').expires_at;
    S.envelopes.set('dev_pixel900', clone(fx('key-envelope').envelope));
    takeSnapshot();
  }
  function base(id, root, parent, name, kind, size) {
    const t = '2026-11-15T14:00:00Z';
    return { id, root_id: root, parent_id: parent, name, kind, size, rev: '1-dir', meta_version: 1, created_at: t, modified_at: t, permissions: permsFor(root) };
  }
  const folder = (id, root, parent, name) => { const b = base(id, root, parent, name, 'folder', 0); if (!parent) delete b.parent_id; return b; };
  function synth(name, size) { const line = Buffer.from(`stub:${name}\n`); const b = Buffer.alloc(size); for (let o = 0; o < size; o += line.length) line.copy(b, o, 0, Math.min(line.length, size - o)); return b; }
  function ensureChunks(buf) {
    const refs = [];
    for (let o = 0; o < buf.length; o += MiB) { const c = Buffer.from(buf.subarray(o, o + MiB)); const h = blake3hex(c); S.chunks.set(h, c); refs.push({ hash: h, offset: o, size: c.length }); }
    return refs;
  }
  function setContent(rec, buf, initial = false) {
    const it = rec.item;
    rec.content = buf; rec.chunks = ensureChunks(buf);
    it.size = buf.length; it.content_hash = blake3hex(buf);
    if (!initial) it.meta_version += 1;
    it.rev = `${it.meta_version}-${it.content_hash}`;
  }
  const policyOf = (rootId) => S.roots.get(rootId)?.policy;
  function permsFor(rootId) {
    const p = policyOf(rootId) || { write: true, shareable: true, agent_process: false };
    return { list: true, read: true, write: p.write, delete: p.write, share: p.shareable, comment: true, agent_process: p.agent_process, transfer: true, execute: false };
  }
  const GUEST_PERMS = {
    view_download: { list: true, read: true, write: false, delete: false, share: false, comment: false, agent_process: false, transfer: true, execute: false },
    upload_only: { list: false, read: false, write: true, delete: false, share: false, comment: false, agent_process: false, transfer: false, execute: false },
    edit: { list: true, read: true, write: true, delete: true, share: false, comment: true, agent_process: false, transfer: true, execute: false },
    comment: { list: true, read: true, write: false, delete: false, share: false, comment: true, agent_process: false, transfer: true, execute: false },
  };
  const isLive = (rec) => { for (let r = rec; r; r = r.item.parent_id ? S.items.get(r.item.parent_id) : null) if (r.state !== 'live') return false; return true; };
  const kids = (id) => [...S.items.values()].filter((r) => r.item.parent_id === id && r.state === 'live');
  const childRoot = (parent) => parent.item.root_link || parent.item.root_id;
  const descends = (rec, ancestorId) => { for (let r = rec, n = 0; r && n < 200; r = r.item.parent_id ? S.items.get(r.item.parent_id) : null, n++) if (r.item.id === ancestorId) return true; return false; };

  function view(rec, ctx) {
    const it = clone(rec.item);
    it.permissions = ctx?.guest ? { ...GUEST_PERMS[ctx.guest.role] } : permsFor(rec.item.root_id);
    if (!it.pinned_on?.length) delete it.pinned_on;
    if (ctx?.guest) delete it.modified_by;
    return it;
  }
  const rootCursor = (rid2) => `c1.${rid2}.${String(S.logs.get(rid2).seq).padStart(6, '0')}`;

  function takeSnapshot() {
    const at = nowIso();
    const roots = [...S.roots.values()].filter((r) => !r.policy.protected);
    const manifest = { schema: 'files-snapshot/1', computer_id: cfg.computerId, written_at: at, encryption: 'none', names_encrypted: false, roots: roots.map((r) => { const c = clone(r); delete c.path_hint; c.cursor = rootCursor(r.id); return c; }) };
    const files = new Map();
    for (const r of roots) {
      const items = [...S.items.values()].filter((x) => x.item.root_id === r.id && x.item.id !== r.root_item_id && isLive(x)).map((x) => {
        const i = x.item; const s = { id: i.id, name: i.name, kind: i.kind, size: i.size, rev: i.rev, modified_at: i.modified_at };
        if (i.parent_id) s.parent_id = i.parent_id; if (i.root_link) s.root_link = i.root_link; return s;
      });
      files.set(r.id, { schema: 'files-snapshot-root/1', root_id: r.id, cursor: rootCursor(r.id), written_at: at, encryption: 'none', names_encrypted: false, items });
    }
    for (const e of validateType('SnapshotManifest', manifest)) throw new Error(`snapshot manifest invalid: ${e}`);
    for (const f of files.values()) for (const e of validateType('SnapshotRootFile', f)) throw new Error(`snapshot root invalid: ${e}`);
    S.snap = { at, manifest, files };
  }
  function snapFind(id) {
    for (const f of S.snap.files.values()) { const s = f.items.find((i) => i.id === id); if (s) return { s, root: f.root_id }; }
    for (const r of S.snap.manifest.roots) if (r.root_item_id === id) return { s: { id, name: r.name, kind: 'folder', size: 0, rev: '1-dir', modified_at: S.snap.at }, root: r.id };
    return null;
  }
  function snapItem(found, ctx) {
    const { s, root } = found;
    const it = { id: s.id, root_id: root, name: s.name, kind: s.kind, size: s.size, rev: s.rev, meta_version: parseInt(s.rev, 10), created_at: s.modified_at, modified_at: s.modified_at, permissions: ctx?.guest ? { ...GUEST_PERMS[ctx.guest.role] } : permsFor(root) };
    if (s.parent_id) it.parent_id = s.parent_id; if (s.root_link) it.root_link = s.root_link;
    if (s.kind === 'file') it.content_hash = s.rev.split('-')[1];
    return it;
  }
  const snapDescends = (id, anc) => { for (let cur = id, n = 0; cur && n < 200; n++) { if (cur === anc) return true; const f = snapFind(cur); cur = f?.s.parent_id; } return false; };

  // ------------------------------------------------------------ change log, events, snapshot, audit
  function emit(ev) {
    const errs = validateType('FilesEvent', ev);
    if (errs.length) throw new Error(`event invalid: ${errs.join('; ')}`);
    S.events.push(ev); if (S.events.length > 500) S.events.shift();
  }
  const evBase = (type) => ({ type, event_id: `ev_${rid(12)}`, at: nowIso(), computer_id: cfg.computerId });
  function record(rootId, op, rec, by, extra = {}) {
    const log = S.logs.get(rootId); if (!log) return;
    log.seq += 1;
    const entry = { seq: log.seq, op, item_id: rec.item.id, at: nowIso(), by };
    if (op !== 'meta' || true) entry.item = view(rec);
    if (extra.old_parent_id) entry.old_parent_id = extra.old_parent_id;
    log.entries.push(entry);
    const root = S.roots.get(rootId); root.cursor = rootCursor(rootId);
    emit({ ...evBase('files.changed'), root_id: rootId, item_id: rec.item.id, op, rev: rec.item.rev, by });
    if (op === 'created' && root.kind === 'inbox') emit({ ...evBase('files.inbox.dropped'), root_id: rootId, item_id: rec.item.id, name: rec.item.name, size: rec.item.size, dropped_by: by, untrusted: true, task_id: `tsk_${rid(8)}`, task_state: 'needs_approval' });
    for (const w of [...waiters]) if (w.root === rootId) { waiters.delete(w); w.resolve(); }
    for (const c of wsClients) c.onChange?.(rootId);
    if (stateOf(S.scenario) === 'awake') takeSnapshot();
  }
  function touch(parentId, by) { const p = S.items.get(parentId); if (p) { p.item.meta_version += 1; p.item.rev = `${p.item.meta_version}-dir`; p.item.modified_at = nowIso(); } }
  function audit(ctx, action, itemId, result, error) {
    if (!ctx.guest) return;
    const row = { share_id: ctx.guest.share_id, guest_id: ctx.guest.guest_id, action, result, at: nowIso() };
    if (itemId) row.item_id = itemId; if (error) row.error = error;
    S.audit.push(row);
    const sh = S.shares.get(ctx.guest.share_id);
    if (sh && result === 'allowed' && action !== 'redeem' && !S.guests.get(ctx.guest.guest_id)?.counted) { sh.guest_count += 1; S.guests.get(ctx.guest.guest_id).counted = true; }
    emit({ ...evBase('files.share.accessed'), access: row });
  }

  // ------------------------------------------------------------ scenarios and computer state
  const stateOf = (scn) => (scn === 'asleep' ? 'asleep' : scn === 'waking' ? 'waking' : 'awake');
  function setScenario(scn) {
    if (!SCENARIOS.includes(scn)) throw new StubError('invalid_request', `unknown scenario ${scn}; use one of ${SCENARIOS.join(', ')}`);
    const before = stateOf(S.scenario);
    S.scenario = scn;
    if (stateOf(scn) === 'awake') takeSnapshot();
    for (const w of [...waiters]) { waiters.delete(w); w.resolve(); }
    if (stateOf(scn) !== before) for (const c of wsClients) c.onState?.(stateOf(scn));
  }
  function startWake() {
    S.touches += 1; // the consent call records exactly one activity touch (SPEC 9.5)
    if (S.scenario === 'asleep') {
      setScenario('waking');
      clearTimeout(wakeTimer);
      wakeTimer = setTimeout(() => setScenario('awake'), cfg.wakeMs);
    }
  }
  const wakeRemainingS = () => 5;

  // ------------------------------------------------------------ errors
  function errBody(code, message, extra = {}) {
    const b = { error: code, code, message: message || MESSAGES[code] || code.replace(/_/g, ' ') };
    const consent = `/api/v1/computer-files/${cfg.computerId}/wake-consent`;
    if (code === 'wake_consent_required' || code === 'wake_denied') { b.computer_state = 'asleep'; b.wake = { setting: S.wakeSetting, consent_path: consent }; }
    if (code === 'waking') { b.retry_after_s = wakeRemainingS(); b.computer_state = 'waking'; }
    if (code === 'asleep' || code === 'guest_wake_not_allowed') b.computer_state = 'asleep';
    if (code === 'offline') b.computer_state = 'offline';
    if (code === 'rate_limited') b.retry_after_s = 3;
    return { ...b, ...extra };
  }
  const E = (code, message, extra) => new StubError(code, message, extra);

  // ------------------------------------------------------------ context + auth
  async function makeCtx(req, url) {
    const h = req.headers;
    const hs = h['x-files-stub-scenario'];
    if (hs && !SCENARIOS.includes(hs)) throw E('invalid_request', `X-Files-Stub-Scenario must be one of ${SCENARIOS.join(', ')}`);
    const scn = hs || S.scenario;
    const ctx = { req, url, q: url.searchParams, scn, h, gateway: url.pathname.startsWith('/api/v1/files-gateway/v2/'), guest: null, hasTicket: false };
    const auth = h.authorization || '';
    if (auth.startsWith('Files-Guest ')) {
      const c = verifyToken(auth.slice(12).trim(), 'files_guest');
      const sh = S.shares.get(c.share_id);
      if (!sh || sh.revoked_at) throw E('share_revoked', 'This share link was turned off.');
      if (sh.expires_at && Date.parse(sh.expires_at) < Date.now()) throw E('share_expired', 'This share link has expired.');
      ctx.guest = { ...c, name: S.guests.get(c.guest_id)?.name };
    } else if (ctx.gateway || scn === 'guest') throw E('unauthorized', 'Authorization: Files-Guest <token> is required here.');
    const tk = h['x-allternit-wake-ticket'];
    if (tk) { try { const c = verifyToken(tk, 'wake'); ctx.hasTicket = c.computer_id === cfg.computerId; } catch { ctx.hasTicket = false; } }
    const st = stateOf(scn);
    ctx.state = st;
    ctx.live = st === 'awake';
    if (!ctx.live && ctx.hasTicket && !ctx.guest) {
      // a wake ticket makes the relay wait for the runtime (up to 30 s) instead of failing
      const t0 = Date.now();
      while (S.scenario === 'waking' && scn === 'waking' && Date.now() - t0 < 30000) await new Promise((r) => { const w = { root: null, resolve: r }; waiters.add(w); setTimeout(() => { waiters.delete(w); r(); }, 250); });
      ctx.state = 'awake'; ctx.live = true; ctx.ticketServed = true;
    }
    return ctx;
  }
  function principal(ctx) {
    if (ctx.guest) return { kind: 'guest', id: ctx.guest.guest_id, display_name: ctx.guest.name || 'Guest' };
    const d = ctx.h['x-allternit-device-id'];
    if (d && /^dev_[A-Za-z0-9_-]{4,64}$/.test(d)) return { kind: 'own_device', id: d, display_name: ctx.h['x-allternit-device-name'] || d };
    return OWNER;
  }
  function needLive(ctx) {
    if (ctx.live) return;
    if (ctx.guest) {
      const sh = S.shares.get(ctx.guest.share_id);
      if (!sh?.guest_wake) { audit(ctx, 'wake', undefined, 'denied', 'guest_wake_not_allowed'); throw E('guest_wake_not_allowed'); }
    }
    if (ctx.state === 'waking') throw E('waking');
    if (S.wakeSetting === 'never') throw E('wake_denied');
    throw E('wake_consent_required');
  }
  function need(ctx, action, itemId) {
    if (!ctx.guest) return;
    if (GUEST_PERMS[ctx.guest.role][action]) return;
    const msg = ctx.guest.role === 'upload_only' ? 'This link only allows uploads.' : 'This link does not allow that.';
    audit(ctx, action === 'list' ? 'list' : action === 'read' ? 'download' : action === 'comment' ? 'comment' : 'upload', itemId, 'denied', 'forbidden');
    throw E('forbidden', msg);
  }
  function scopeRec(ctx, rec) {
    if (ctx.guest && !descends(rec, ctx.guest.item_id)) { audit(ctx, 'list', rec.item.id, 'denied', 'scope_violation'); throw E('scope_violation', 'That item is outside this link.'); }
  }
  function getRoot(ctx, id) {
    const r = S.roots.get(id);
    if (!r) throw E('root_not_found', `No root ${id}.`);
    if (ctx.guest && ctx.guest.root_id !== id) throw E('scope_violation', 'That root is outside this link.');
    if (r.policy.protected && !ctx.h['x-allternit-unlock']) throw E('protected_root_locked', 'This root needs a device unlock (Face ID / biometrics).');
    return r;
  }
  function getLive(id) {
    const rec = S.items.get(id);
    if (!rec || !isLive(rec)) throw E('not_found', `No item ${id}.`);
    return rec;
  }
  function guardItem(ctx, rec) {
    const r = S.roots.get(rec.item.root_id);
    if (r?.policy.protected && !ctx.h['x-allternit-unlock']) throw E('protected_root_locked', 'This root needs a device unlock (Face ID / biometrics).');
    scopeRec(ctx, rec);
  }
  const fresh = (ctx) => (ctx.live ? { served_by: 'live', computer_state: 'awake' } : { served_by: 'snapshot', computer_state: ctx.state, snapshot_at: S.snap.at });
  function parseBody(ctx, buf, type) {
    let o;
    try { o = JSON.parse(buf.toString('utf8') || 'null'); } catch { throw E('invalid_request', 'Body is not valid JSON.'); }
    const errs = validateType(type, o);
    if (errs.length) throw E('invalid_request', errs.slice(0, 3).join('; '));
    return o;
  }
  function qint(ctx, name, def, min, max) {
    const v = ctx.q.get(name); if (v === null) return def;
    const n = Number(v);
    if (!Number.isInteger(n) || n < min || n > max) throw E('invalid_request', `${name} must be an integer ${min}..${max}`);
    return n;
  }
  const pageOf = (tok) => { if (!tok) return 0; try { return JSON.parse(Buffer.from(tok.slice(3), 'base64url').toString()).o | 0; } catch { throw E('invalid_request', 'Bad page_token.'); } };
  const pageTok = (o) => `p1.${b64u(JSON.stringify({ o }))}`;
  function checkName(name) { if (!name || name === '.' || name === '..' || /[\/\\\0]/.test(name) || name.length > 255) throw E('invalid_name', `"${name}" is not a valid name.`); }
  function uniqueName(parentId, name, exceptId) {
    const taken = new Set(kids(parentId).filter((r) => r.item.id !== exceptId).map((r) => r.item.name));
    if (!taken.has(name)) return name;
    const [stem, ext] = splitExt(name);
    for (let n = 2; ; n++) if (!taken.has(`${stem} (${n})${ext}`)) return `${stem} (${n})${ext}`;
  }
  const byName = (parentId, name) => kids(parentId).find((r) => r.item.name === name);

  // ------------------------------------------------------------ writes
  function newFile(rootId, parentId, name, buf, by, mtime, chunks) {
    const t = mtime || nowIso();
    const it = { id: `it_${rid(12)}`, root_id: rootId, parent_id: parentId, name, kind: 'file', size: 0, rev: '1-dir', meta_version: 1, created_at: t, modified_at: t, modified_by: by, mime: mimeOf(name), permissions: permsFor(rootId) };
    if (by.kind === 'guest') it.provenance = { source: 'guest', ai_generated: false };
    const rec = { item: it, content: null, chunks: [], state: 'live' };
    setContent(rec, buf, true);
    if (chunks) rec.chunks = chunks;
    S.items.set(it.id, rec);
    touch(parentId, by);
    record(rootId, 'created', rec, by);
    return rec;
  }
  /** SPEC 4: date and HHmm in the editing device's local time (utc_offset_minutes), else UTC. */
  function conflictName(name, device, utcOffsetMinutes) {
    const d = new Date(Date.now() + (Number.isInteger(utcOffsetMinutes) ? utcOffsetMinutes : 0) * 60000); const p = (n) => String(n).padStart(2, '0');
    const [stem, ext] = splitExt(name);
    return `${stem} (conflict ${device} ${d.getUTCFullYear()}-${p(d.getUTCMonth() + 1)}-${p(d.getUTCDate())} ${p(d.getUTCHours())}${p(d.getUTCMinutes())})${ext}`;
  }
  /** Shared by the small write and upload commit. Returns {item, conflict?, cursor}. */
  function writeFile(ctx, o) {
    const by = principal(ctx);
    const parent = getLive(o.parent_id);
    if (parent.item.kind !== 'folder') throw E('invalid_request', 'parent_id is not a folder.');
    const rootId = childRoot(parent);
    const pol = policyOf(rootId);
    if (pol && !pol.write) throw E('forbidden', 'This root is read-only.');
    checkName(o.name);
    let existing = o.item_id ? S.items.get(o.item_id) : byName(o.parent_id, o.name);
    if (o.item_id && (!existing || !isLive(existing))) {
      // SPEC 4: base_rev names a file that is now gone. keep_both creates it again (as a create); fail and replace: not_found.
      if (o.on_conflict !== 'keep_both') throw E('not_found', `No item ${o.item_id}.`);
      const r = newFile(rootId, o.parent_id, uniqueName(o.parent_id, o.name), o.buf, by, o.modified_at, o.chunks);
      return { item: view(r, ctx), cursor: rootCursor(rootId) };
    }
    if (o.item_id && existing.item.kind !== 'file') throw E('invalid_request', 'item_id is a folder.');
    if (!existing && o.base_rev && o.on_conflict !== 'keep_both') throw E('not_found', `"${o.name}" is gone; send on_conflict=keep_both to create it again.`);
    if (!existing) { const r = newFile(rootId, o.parent_id, o.name, o.buf, by, o.modified_at, o.chunks); return { item: view(r, ctx), cursor: rootCursor(rootId) }; }
    const cur = existing.item;
    if (!o.base_rev) {
      // a name clash with no base_rev: only a new-file write may land here
      if (o.item_id) throw E('invalid_request', 'base_rev is required when replacing.');
      if (o.on_conflict === 'fail') throw E('already_exists', `"${o.name}" already exists.`, { current: view(existing, ctx) });
      if (o.on_conflict === 'keep_both') { const r = newFile(rootId, o.parent_id, uniqueName(o.parent_id, o.name), o.buf, by, o.modified_at, o.chunks); return { item: view(r, ctx), cursor: rootCursor(rootId) }; }
      return replaceContent(existing, o, by, rootId, ctx);
    }
    const stale = o.base_rev !== cur.rev || ctx.scn === 'conflict';
    if (!stale) return replaceContent(existing, o, by, rootId, ctx);
    if (o.on_conflict === 'keep_both') {
      const device = o.device_name || by.display_name || 'device';
      const cname = conflictName(cur.name, device, o.utc_offset_minutes);
      const copy = newFile(rootId, o.parent_id, uniqueName(o.parent_id, cname), o.buf, by, o.modified_at, o.chunks);
      const conflict = { kind: 'keep_both', original_item_id: cur.id, base_rev: o.base_rev, current_rev: cur.rev, conflict_item: view(copy, ctx), conflict_name: copy.item.name };
      emit({ ...evBase('files.conflict'), conflict });
      return { item: view(existing, ctx), conflict, cursor: rootCursor(rootId) };
    }
    throw E('rev_mismatch', undefined, { current: view(existing, ctx) });
  }
  function replaceContent(rec, o, by, rootId, ctx) {
    const it = rec.item;
    S.versions.set(it.id, [...(S.versions.get(it.id) || []), { v: { version_id: `v_${S.vSeq++}`, rev: it.rev, size: it.size, content_hash: it.content_hash, created_at: it.modified_at, created_by: it.modified_by || OWNER }, content: rec.content }]);
    setContent(rec, o.buf);
    if (o.chunks) rec.chunks = o.chunks;
    it.modified_at = o.modified_at || nowIso(); it.modified_by = by;
    record(rootId, 'modified', rec, by);
    return { item: view(rec, ctx), cursor: rootCursor(rootId) };
  }

  // ------------------------------------------------------------ routes
  const R = (status, body, type, headers) => ({ status, body, type, headers });
  const routes = [];
  const V2 = '(?:/api/v1/files/v2|/api/v1/files-gateway/v2)';
  const route = (method, p, fn, o = {}) => routes.push({ method, re: new RegExp(`^${p.startsWith('^') ? p.slice(1) : V2 + p}/?$`), fn, ...o });
  const cloud = (method, p, fn, o = {}) => routes.push({ method, re: new RegExp(`^/api/v1/${p}/?$`), fn, cloud: true, ...o });

  route('GET', '/ping', (ctx) => R(200, { computer_id: cfg.computerId, protocol: 'files/2', runtime_version: 'files-stub-0.1.0', computer_state: ctx.state }, 'PingResponse'));
  route('GET', '/whoami', (ctx, _p, body) => {
    if (ctx.h['x-files-stub-principal'] === 'agent') { const w = clone(fx('whoami-agent')); w.computer_id = cfg.computerId; return R(200, w, 'WhoAmIResponse'); }
    const grants = ctx.guest ? [{ root_id: ctx.guest.root_id, permissions: { ...GUEST_PERMS[ctx.guest.role] }, item_scope: ctx.guest.item_id }] : [...S.roots.values()].map((r) => ({ root_id: r.id, permissions: permsFor(r.id) }));
    return R(200, { principal: principal(ctx), computer_id: cfg.computerId, grants, freshness: fresh(ctx) }, 'WhoAmIResponse');
  });
  route('GET', '/roots', (ctx) => {
    let roots;
    if (ctx.live) roots = [...S.roots.values()].map((r) => { const c = clone(r); c.cursor = rootCursor(r.id); c.usage_bytes = [...S.items.values()].filter((x) => x.item.root_id === r.id && x.item.kind === 'file' && isLive(x)).reduce((a, x) => a + x.item.size, 0); return c; });
    else roots = clone(S.snap.manifest.roots);
    if (ctx.guest) roots = roots.filter((r) => r.id === ctx.guest.root_id).map((r) => { delete r.path_hint; return r; });
    return R(200, { computer_id: cfg.computerId, roots, freshness: fresh(ctx) }, 'RootsResponse');
  });
  route('POST', '/roots', (ctx, _p, body) => {
    needLive(ctx); if (ctx.guest) throw E('forbidden', 'Guests cannot add roots.');
    const rq = parseBody(ctx, body, 'AddRootRequest');
    const lp = path.resolve(rq.local_path.replace(/^~(?=\/|$)/, os.homedir()));
    if (!lp.startsWith(os.homedir() + path.sep) || /(^|\/)(\.ssh|\.gnupg|Library\/Keychains|\.aws|\.config\/allternit)(\/|$)/.test(lp)) throw E('scope_violation', 'That folder is outside the home folder or on the secrets deny-list.');
    const relayed = !!ctx.h['x-allternit-relayed'];
    const id = `rt_${rid(8)}`; const itemId = `it_${rid(12)}`;
    const policy = { ...rq.policy, ...(relayed ? { shareable: false, agent_process: false } : {}) };
    const root = { id: id, kind: 'user_added', name: rq.name, path_hint: rq.local_path, policy, root_item_id: itemId, cursor: `c1.${id}.000000`, provider_ready: true };
    S.roots.set(id, root); S.logs.set(id, { base: 0, seq: 0, entries: [] });
    S.items.set(itemId, { item: folder(itemId, id, undefined, rq.name), state: 'live', content: null, chunks: [] });
    return R(201, { root: clone(root) }, 'RootResponse');
  });
  route('PATCH', '/roots/([^/]+)', (ctx, p, body) => {
    needLive(ctx); const r = getRoot(ctx, p[0]); const rq = parseBody(ctx, body, 'UpdateRootRequest');
    if (rq.name) r.name = rq.name; if (rq.policy) r.policy = rq.policy;
    return R(200, { root: clone(r) }, 'RootResponse');
  });
  route('DELETE', '/roots/([^/]+)', (ctx, p) => {
    needLive(ctx); const r = getRoot(ctx, p[0]);
    if (r.kind !== 'user_added') throw E('forbidden', 'Only user-added roots can be removed.');
    S.roots.delete(r.id); return R(204);
  });
  route('GET', '/roots/([^/]+)/list', (ctx, p) => {
    const root = getRoot(ctx, p[0]);
    const lq = { parent_id: ctx.q.get('parent_id') ?? undefined };
    for (const k of ['page_token', 'sort']) if (ctx.q.get(k) !== null) lq[k] = ctx.q.get(k);
    if (ctx.q.get('limit') !== null) lq.limit = Number(ctx.q.get('limit'));
    const errs = validateType('ListQuery', lq); if (errs.length) throw E('invalid_request', errs.slice(0, 3).join('; '));
    const limit = Math.min(lq.limit || 200, 1000); const off = pageOf(lq.page_token); const sort = lq.sort || 'name';
    const cmp = (a, b) => (a.kind !== b.kind ? (a.kind === 'folder' ? -1 : 1) : sort === 'modified' ? b.modified_at.localeCompare(a.modified_at) : sort === 'size' ? b.size - a.size : a.name.localeCompare(b.name, undefined, { sensitivity: 'base' }));
    let parent, items;
    if (ctx.live) {
      const rec = getLive(lq.parent_id); guardItem(ctx, rec);
      if (rec.item.kind !== 'folder') throw E('invalid_request', 'parent_id is not a folder.');
      need(ctx, 'list', rec.item.id); audit(ctx, 'list', rec.item.id, 'allowed');
      parent = view(rec, ctx); items = kids(rec.item.id).map((r) => view(r, ctx));
    } else {
      const f = snapFind(lq.parent_id); if (!f) throw E('not_found', `No item ${lq.parent_id}.`);
      if (ctx.guest && !snapDescends(lq.parent_id, ctx.guest.item_id)) throw E('scope_violation', 'That item is outside this link.');
      need(ctx, 'list', lq.parent_id);
      parent = snapItem(f, ctx); items = [...S.snap.files.values()].flatMap((x) => x.items).filter((s) => s.parent_id === lq.parent_id).map((s) => snapItem(snapFind(s.id), ctx));
      if (ctx.guest) audit(ctx, 'list', lq.parent_id, 'allowed');
    }
    items.sort(cmp);
    const page = items.slice(off, off + limit);
    const out = { root_id: root.id, parent, items: page, cursor: ctx.live ? rootCursor(root.id) : S.snap.files.get(root.id)?.cursor || root.cursor, freshness: fresh(ctx) };
    if (off + limit < items.length) out.next_page_token = pageTok(off + limit);
    return R(200, out, 'ListResponse');
  });
  route('GET', '/items/([^/]+)', (ctx, p) => {
    if (ctx.live) { const rec = getLive(p[0]); guardItem(ctx, rec); need(ctx, 'list', p[0]); audit(ctx, 'list', p[0], 'allowed'); return R(200, { item: view(rec, ctx), freshness: fresh(ctx) }, 'StatResponse'); }
    const f = snapFind(p[0]); if (!f) throw E('not_found', `No item ${p[0]}.`);
    if (ctx.guest && !snapDescends(p[0], ctx.guest.item_id)) throw E('scope_violation', 'That item is outside this link.');
    need(ctx, 'list', p[0]);
    return R(200, { item: snapItem(f, ctx), freshness: fresh(ctx) }, 'StatResponse');
  });
  route('GET', '/items/([^/]+)/content', (ctx, p) => {
    needLive(ctx); const rec = getLive(p[0]); guardItem(ctx, rec); need(ctx, 'read', p[0]);
    if (rec.item.kind !== 'file') throw E('invalid_request', 'That item is a folder.');
    audit(ctx, 'download', p[0], 'allowed');
    const etag = `"${rec.item.rev}"`;
    const h = { ETag: etag, 'Content-Type': rec.item.mime || 'application/octet-stream', 'Accept-Ranges': 'bytes', 'X-Files-Served-By': 'live' };
    if (ctx.h['if-none-match'] && ctx.h['if-none-match'].replace(/^W\//, '') === etag) return { status: 304, headers: h };
    const buf = rec.content; const rg = ctx.h.range;
    if (rg) {
      const m = /^bytes=(\d*)-(\d*)$/.exec(rg);
      if (!m || (m[1] === '' && m[2] === '')) throw E('invalid_request', 'Bad Range header.');
      let a = m[1] === '' ? Math.max(0, buf.length - Number(m[2])) : Number(m[1]); let b = m[1] === '' || m[2] === '' ? buf.length - 1 : Math.min(Number(m[2]), buf.length - 1);
      if (a > b || a >= buf.length) throw E('invalid_request', 'Range not satisfiable.');
      return { status: 206, bytes: buf.subarray(a, b + 1), headers: { ...h, 'Content-Range': `bytes ${a}-${b}/${buf.length}` } };
    }
    return { status: 200, bytes: buf, headers: h };
  });
  route('GET', '/items/([^/]+)/manifest', (ctx, p) => {
    needLive(ctx); const rec = getLive(p[0]); guardItem(ctx, rec); need(ctx, 'read', p[0]);
    if (rec.item.kind !== 'file') throw E('invalid_request', 'That item is a folder.');
    return R(200, { item_id: p[0], rev: rec.item.rev, size: rec.item.size, content_hash: rec.item.content_hash, chunker: { algo: 'fastcdc-2020', hash: 'blake3', min_bytes: 262144, avg_bytes: 1048576, max_bytes: MAX_CHUNK }, chunks: rec.chunks }, 'Manifest');
  });
  route('GET', '/chunks/([^/]+)', (ctx, p) => {
    needLive(ctx); if (ctx.guest) need(ctx, 'read');
    if (!/^[0-9a-f]{64}$/.test(p[0])) throw E('invalid_request', 'Bad chunk hash.');
    const c = S.chunks.get(p[0]); if (!c) throw E('not_found', 'No such chunk.');
    return { status: 200, bytes: c, headers: { 'Content-Type': 'application/octet-stream' } };
  });
  route('PUT', '/roots/([^/]+)/content', async (ctx, p, body) => {
    const root = getRoot(ctx, p[0]);
    if (!ctx.live && ctx.guest && ctx.guest.role === 'upload_only' && ctx.scn === 'asleep') {
      // SPEC 9.6: an upload_only drop to a sleeping computer is queued in the owner's own bucket
      const name = ctx.q.get('name'); checkName(name);
      const d = { drop_id: `drp_${rid(6)}`, share_id: ctx.guest.share_id, guest_id: ctx.guest.guest_id, name, size: body.length, content_hash: blake3hex(body), r2_key: `files-drops/${cfg.computerId}/drp_${rid(6)}`, received_at: nowIso(), state: 'queued' };
      S.drops.push(d); audit(ctx, 'upload', undefined, 'allowed'); return R(202, d, 'QueuedDrop');
    }
    needLive(ctx);
    if (body.length > MAX_CHUNK) throw E('payload_too_large', 'Small writes are 4 MiB at most; use an upload session.');
    const parent_id = ctx.q.get('parent_id'), name = ctx.q.get('name'); const on_conflict = ctx.q.get('on_conflict') || 'keep_both';
    if (!parent_id || !name) throw E('invalid_request', 'parent_id and name are required.');
    if (!['fail', 'keep_both', 'replace'].includes(on_conflict)) throw E('invalid_request', 'on_conflict must be fail, keep_both or replace.');
    const pr = getLive(parent_id); guardItem(ctx, pr); need(ctx, 'write', parent_id);
    audit(ctx, 'upload', parent_id, 'allowed');
    const br = ctx.q.get('base_rev') || undefined;
    if (br && validateType('Rev', br).length) throw E('invalid_request', 'Bad base_rev.');
    const off = ctx.q.get('utc_offset_minutes'); const utc_offset_minutes = off == null ? undefined : Number(off);
    if (utc_offset_minutes !== undefined && (!Number.isInteger(utc_offset_minutes) || Math.abs(utc_offset_minutes) > 840)) throw E('invalid_request', 'utc_offset_minutes must be an integer from -840 to 840.');
    const res = writeFile(ctx, { parent_id, name, base_rev: br, on_conflict, buf: body, device_name: ctx.q.get('device_name') || ctx.h['x-allternit-device-name'], utc_offset_minutes });
    return R(res.conflict ? 200 : 200, res, 'MutationResponse');
  }, { body: 4 * MiB + 65536 });
  route('POST', '/chunks/have-need', (ctx, _p, body) => {
    needLive(ctx); const rq = parseBody(ctx, body, 'HaveNeedRequest');
    return R(200, { have: rq.hashes.filter((h) => S.chunks.has(h)), need: rq.hashes.filter((h) => !S.chunks.has(h)) }, 'HaveNeedResponse');
  });
  const upStatus = (u) => {
    const uniq = [...new Set(u.refs.map((r) => r.hash))];
    return { session_id: u.id, state: u.state, need: uniq.filter((h) => !S.chunks.has(h)), received_bytes: uniq.filter((h) => S.chunks.has(h)).reduce((a, h) => a + S.chunks.get(h).length, 0), total_bytes: u.req.size, expires_at: nowIso(u.expires), max_request_bytes: 5242880 };
  };
  const getUp = (ctx, id) => {
    const u = S.uploads.get(id); if (!u) throw E('upload_session_not_found', `No upload session ${id}.`);
    if (u.guest !== (ctx.guest?.guest_id || null)) throw E('upload_session_not_found', `No upload session ${id}.`);
    if (u.state === 'open' && u.expires < Date.now()) { u.state = 'expired'; }
    if (u.state === 'expired') throw E('upload_session_expired', 'This upload session expired. Start a new one.');
    return u;
  };
  route('POST', '/uploads', (ctx, _p, body) => {
    needLive(ctx); const rq = parseBody(ctx, body, 'UploadCreateRequest');
    getRoot(ctx, rq.root_id); const pr = getLive(rq.parent_id); guardItem(ctx, pr); need(ctx, 'write', rq.parent_id);
    if (pr.item.kind !== 'folder') throw E('invalid_request', 'parent_id is not a folder.');
    if (rq.item_id && !rq.base_rev) throw E('invalid_request', 'base_rev is required when replacing.');
    if (rq.chunks.length > rq.total_chunks) throw E('manifest_invalid', 'More chunks than total_chunks.');
    checkName(rq.name);
    const u = { id: `up_${rid(12)}`, req: rq, refs: [...rq.chunks], state: 'open', expires: Date.now() + cfg.uploadTtlS * 1000, guest: ctx.guest?.guest_id || null };
    S.uploads.set(u.id, u); return R(201, upStatus(u), 'UploadStatus');
  });
  route('POST', '/uploads/([^/]+)/chunks', (ctx, p, body) => {
    needLive(ctx); const u = getUp(ctx, p[0]); const rq = parseBody(ctx, body, 'UploadChunksAppend');
    if (u.state !== 'open') throw E('invalid_request', `Session is ${u.state}.`);
    if (u.refs.length + rq.chunks.length > u.req.total_chunks) throw E('manifest_invalid', 'More chunks than total_chunks.');
    u.refs.push(...rq.chunks); return R(200, upStatus(u), 'UploadStatus');
  });
  route('PUT', '/uploads/([^/]+)/chunks/([^/]+)', async (ctx, p, body) => {
    needLive(ctx); const u = getUp(ctx, p[0]);
    if (body.length > MAX_CHUNK) throw E('payload_too_large', 'A chunk is 4 MiB at most (4194304 bytes).');
    if (!/^[0-9a-f]{64}$/.test(p[1])) throw E('invalid_request', 'Bad chunk hash.');
    const ref = u.refs.find((r) => r.hash === p[1]);
    if (!ref) throw E('invalid_request', 'That chunk is not in this session. Append it first.');
    if (body.length !== ref.size || blake3hex(body) !== p[1]) throw E('chunk_hash_mismatch', 'The chunk bytes do not hash to the hash in the URL (or differ from the declared size).');
    const had = S.chunks.has(p[1]); if (!had) S.chunks.set(p[1], Buffer.from(body));
    return R(200, { hash: p[1], stored: !had, remaining: upStatus(u).need.length }, 'ChunkPutResponse');
  }, { body: 4 * MiB + 65536 });
  route('GET', '/uploads/([^/]+)', (ctx, p) => { needLive(ctx); return R(200, upStatus(getUp(ctx, p[0])), 'UploadStatus'); });
  route('POST', '/uploads/([^/]+)/commit', (ctx, p) => {
    needLive(ctx); const u = getUp(ctx, p[0]);
    if (u.state === 'committed') throw E('invalid_request', 'Already committed.');
    const rq = u.req;
    if (u.refs.length !== rq.total_chunks) throw E('manifest_invalid', `Have ${u.refs.length} chunk refs, expected total_chunks ${rq.total_chunks}.`);
    let off = 0; for (const r of u.refs) { if (r.offset !== off) throw E('manifest_invalid', 'Chunk offsets are not contiguous.'); off += r.size; }
    if (off !== rq.size) throw E('manifest_invalid', 'Chunk sizes do not add up to size.');
    const missing = [...new Set(u.refs.map((r) => r.hash))].filter((h) => !S.chunks.has(h));
    if (missing.length) throw E('chunk_missing', undefined, { missing_chunks: missing.slice(0, 1000) });
    const buf = Buffer.concat(u.refs.map((r) => S.chunks.get(r.hash)));
    if (blake3hex(buf) !== rq.content_hash) throw E('manifest_invalid', 'The assembled file does not hash to content_hash.');
    u.state = 'committing';
    try {
      audit(ctx, 'upload', rq.parent_id, 'allowed');
      const res = writeFile(ctx, { parent_id: rq.parent_id, name: rq.name, item_id: rq.item_id, base_rev: rq.base_rev, on_conflict: rq.on_conflict, buf, chunks: u.refs.map((r) => ({ ...r })), modified_at: rq.modified_at, device_name: rq.device_name, utc_offset_minutes: rq.utc_offset_minutes });
      u.state = 'committed'; return R(200, res, 'UploadCommitResponse');
    } catch (e) { u.state = 'open'; throw e; }
  });
  route('DELETE', '/uploads/([^/]+)', (ctx, p) => { needLive(ctx); getUp(ctx, p[0]); S.uploads.delete(p[0]); return R(204); });

  const placeChild = (ctx, rec, parentId, name, onConflict, by) => {
    const clash = byName(parentId, name);
    if (clash && clash.item.id !== rec.item.id) {
      if (onConflict === 'fail') throw E('already_exists', `"${name}" already exists.`, { current: view(clash, ctx) });
      if (onConflict === 'keep_both') name = uniqueName(parentId, name, rec.item.id);
      else trashRec(clash, by, ctx);
    }
    return name;
  };
  route('POST', '/mkdir', (ctx, _p, body) => {
    needLive(ctx); const rq = parseBody(ctx, body, 'MkdirRequest');
    const pr = getLive(rq.parent_id); guardItem(ctx, pr); need(ctx, 'write', rq.parent_id);
    if (pr.item.kind !== 'folder') throw E('invalid_request', 'parent_id is not a folder.');
    checkName(rq.name); const by = principal(ctx);
    const name = placeChild(ctx, { item: { id: '' } }, rq.parent_id, rq.name, rq.on_conflict, by);
    const rootId = childRoot(pr); const t = nowIso();
    const it = { id: `it_${rid(12)}`, root_id: rootId, parent_id: rq.parent_id, name, kind: 'folder', size: 0, rev: '1-dir', meta_version: 1, created_at: t, modified_at: t, modified_by: by, permissions: permsFor(rootId) };
    const rec = { item: it, state: 'live', content: null, chunks: [] }; S.items.set(it.id, rec); touch(rq.parent_id, by); record(rootId, 'created', rec, by);
    return R(201, { item: view(rec, ctx), cursor: rootCursor(rootId) }, 'MutationResponse');
  });
  route('POST', '/move', (ctx, _p, body) => {
    needLive(ctx); const rq = parseBody(ctx, body, 'MoveRequest');
    const rec = getLive(rq.item_id); guardItem(ctx, rec); need(ctx, 'write', rq.item_id);
    if (S.roots.get(rec.item.root_id)?.root_item_id === rec.item.id) throw E('forbidden', 'A root folder cannot be moved.');
    if (rq.base_rev !== rec.item.rev || ctx.scn === 'conflict') throw E('rev_mismatch', undefined, { current: view(rec, ctx) });
    if (!rq.new_parent_id && !rq.new_name) throw E('invalid_request', 'Give new_parent_id and/or new_name.');
    const by = principal(ctx); const oldParent = rec.item.parent_id;
    const np = rq.new_parent_id ? getLive(rq.new_parent_id) : S.items.get(oldParent);
    if (np.item.kind !== 'folder' || descends(np, rec.item.id)) throw E('invalid_request', 'Cannot move into that folder.');
    if (rq.new_parent_id) guardItem(ctx, np);
    const name = placeChild(ctx, rec, np.item.id, rq.new_name || rec.item.name, rq.on_conflict, by);
    if (rq.new_name) checkName(rq.new_name);
    const moved = np.item.id !== oldParent; const oldRoot = rec.item.root_id;
    rec.item.name = name; rec.item.parent_id = np.item.id; rec.item.root_id = childRoot(np);
    rec.item.meta_version += 1; rec.item.rev = rec.item.kind === 'folder' ? `${rec.item.meta_version}-dir` : `${rec.item.meta_version}-${rec.item.content_hash}`; rec.item.modified_at = nowIso();
    touch(np.item.id, by); if (moved) touch(oldParent, by);
    record(rec.item.root_id, moved ? 'moved' : 'renamed', rec, by, { old_parent_id: moved ? oldParent : undefined });
    if (oldRoot !== rec.item.root_id) record(oldRoot, 'moved', rec, by, { old_parent_id: oldParent });
    return R(200, { item: view(rec, ctx), cursor: rootCursor(rec.item.root_id) }, 'MutationResponse');
  });
  const copyRec = (src, parentId, name, by, ctx) => {
    const pr = S.items.get(parentId); const rootId = childRoot(pr);
    if (src.item.kind === 'file') { const r = newFile(rootId, parentId, name, src.content, by, undefined, src.chunks.map((c) => ({ ...c }))); return r; }
    const t = nowIso(); const it = { id: `it_${rid(12)}`, root_id: rootId, parent_id: parentId, name, kind: 'folder', size: 0, rev: '1-dir', meta_version: 1, created_at: t, modified_at: t, modified_by: by, permissions: permsFor(rootId) };
    const rec = { item: it, state: 'live', content: null, chunks: [] }; S.items.set(it.id, rec); touch(parentId, by); record(rootId, 'created', rec, by);
    for (const k of kids(src.item.id)) copyRec(k, it.id, k.item.name, by, ctx);
    return rec;
  };
  const totalSize = (rec) => (rec.item.kind === 'file' ? rec.item.size : kids(rec.item.id).reduce((a, k) => a + totalSize(k), 0));
  route('POST', '/copy', (ctx, _p, body) => {
    needLive(ctx); const rq = parseBody(ctx, body, 'CopyRequest');
    const src = getLive(rq.item_id); guardItem(ctx, src); need(ctx, 'read', rq.item_id);
    const size = totalSize(src); const by = principal(ctx);
    const mkJob = (kind) => { const j = { job_id: `job_${rid(8)}`, kind, created: Date.now(), total: Math.max(size, 1) }; S.jobs.set(j.job_id, j); return j; };
    if (rq.dest_computer_id && rq.dest_computer_id !== cfg.computerId) { const j = mkJob('copy'); return R(202, { job: jobRef(j) }, 'CopyResponse'); }
    const dp = getLive(rq.dest_parent_id); need(ctx, 'write', rq.dest_parent_id);
    if (dp.item.kind !== 'folder' || descends(dp, src.item.id)) throw E('invalid_request', 'Cannot copy into that folder.');
    checkName(rq.new_name || src.item.name);
    const name = placeChild(ctx, { item: { id: '' } }, dp.item.id, rq.new_name || src.item.name, rq.on_conflict, by);
    const rec = copyRec(src, dp.item.id, name, by, ctx);
    if (size > 64 * MiB) { const j = mkJob('copy'); return R(202, { job: jobRef(j), item: view(rec, ctx), cursor: rootCursor(childRoot(dp)) }, 'CopyResponse'); }
    return R(200, { item: view(rec, ctx), cursor: rootCursor(childRoot(dp)) }, 'CopyResponse');
  });
  const jobRef = (j) => { const age = Date.now() - j.created; const state = age < 300 ? 'queued' : age < 2000 ? 'running' : 'done'; return { job_id: j.job_id, kind: j.kind, state, done_bytes: state === 'done' ? j.total : state === 'running' ? Math.floor(j.total * (age / 2000)) : 0, total_bytes: j.total }; };
  route('GET', '/jobs/([^/]+)', (ctx, p) => { const j = S.jobs.get(p[0]); if (!j) throw E('not_found', `No job ${p[0]}.`); return R(200, jobRef(j), 'JobRef'); });
  const trashRec = (rec, by, ctx) => {
    const rootId = rec.item.root_id; const pol = policyOf(rootId);
    const e = { trash_id: `tr_${rid(8)}`, item: view(rec, ctx), original_parent_id: rec.item.parent_id, trashed_at: nowIso(), trashed_by: by, purge_at: nowIso(Date.now() + (pol?.trash_days ?? 30) * 86400000) };
    rec.state = 'trashed'; rec.trashId = e.trash_id; S.trash.set(e.trash_id, e);
    touch(rec.item.parent_id, by); record(rootId, 'trashed', rec, by); return e;
  };
  const purge = (rec) => { for (const k of [...S.items.values()].filter((r) => r.item.parent_id === rec.item.id)) purge(k); S.items.delete(rec.item.id); };
  route('POST', '/delete', (ctx, _p, body) => {
    needLive(ctx); const rq = parseBody(ctx, body, 'DeleteRequest');
    const rec = getLive(rq.item_id); guardItem(ctx, rec); need(ctx, 'delete', rq.item_id);
    if (S.roots.get(rec.item.root_id)?.root_item_id === rec.item.id || rec.item.root_link) throw E('forbidden', 'A root folder cannot be deleted.');
    if ((rq.base_rev && rq.base_rev !== rec.item.rev) || (rq.base_rev && ctx.scn === 'conflict')) throw E('rev_mismatch', undefined, { current: view(rec, ctx) });
    const by = principal(ctx); const rootId = rec.item.root_id;
    if (rq.permanent) { const v = view(rec, ctx); const par = rec.item.parent_id; purge(rec); touch(par, by); const log = S.logs.get(rootId); rec.state = 'trashed'; record(rootId, 'deleted', { item: v }, by); return R(200, { cursor: rootCursor(rootId) }, 'DeleteResponse'); }
    const e = trashRec(rec, by, ctx);
    return R(200, { trashed: e, cursor: rootCursor(rootId) }, 'DeleteResponse');
  });
  route('GET', '/roots/([^/]+)/trash', (ctx, p) => {
    needLive(ctx); const root = getRoot(ctx, p[0]); if (ctx.guest) throw E('forbidden', 'Guests cannot see trash.');
    const all = [...S.trash.values()].filter((e) => S.items.get(e.item.id)?.state === 'trashed' && e.item.root_id === root.id);
    const off = pageOf(ctx.q.get('page_token')); const out = { root_id: root.id, entries: all.slice(off, off + 200), freshness: fresh(ctx) };
    if (off + 200 < all.length) out.next_page_token = pageTok(off + 200);
    return R(200, out, 'TrashListResponse');
  });
  route('POST', '/restore', (ctx, _p, body) => {
    needLive(ctx); const rq = parseBody(ctx, body, 'RestoreRequest');
    const e = S.trash.get(rq.trash_id); const rec = e && S.items.get(e.item.id);
    if (!rec || rec.state !== 'trashed') throw E('not_found', `No trash entry ${rq.trash_id}.`);
    guardItem(ctx, rec); need(ctx, 'write'); const by = principal(ctx);
    let parent = S.items.get(e.original_parent_id);
    if (!parent || !isLive(parent)) parent = S.items.get(S.roots.get(rec.item.root_id).root_item_id);
    rec.item.name = placeChild(ctx, rec, parent.item.id, rec.item.name, rq.on_conflict, by); rec.item.parent_id = parent.item.id; rec.state = 'live'; S.trash.delete(rq.trash_id);
    rec.item.meta_version += 1; rec.item.rev = rec.item.kind === 'folder' ? `${rec.item.meta_version}-dir` : `${rec.item.meta_version}-${rec.item.content_hash}`;
    touch(parent.item.id, by); record(rec.item.root_id, 'restored', rec, by);
    return R(200, { item: view(rec, ctx), cursor: rootCursor(rec.item.root_id) }, 'MutationResponse');
  });
  route('GET', '/items/([^/]+)/versions', (ctx, p) => {
    needLive(ctx); const rec = getLive(p[0]); guardItem(ctx, rec); need(ctx, 'read', p[0]);
    return R(200, { item_id: p[0], versions: (S.versions.get(p[0]) || []).map((x) => x.v).reverse(), freshness: fresh(ctx) }, 'VersionsResponse');
  });
  route('POST', '/items/([^/]+)/versions/restore', (ctx, p, body) => {
    needLive(ctx); const rq = parseBody(ctx, body, 'RestoreVersionRequest');
    const rec = getLive(p[0]); guardItem(ctx, rec); need(ctx, 'write', p[0]);
    const v = (S.versions.get(p[0]) || []).find((x) => x.v.version_id === rq.version_id); if (!v) throw E('not_found', `No version ${rq.version_id}.`);
    if (rq.base_rev !== rec.item.rev || ctx.scn === 'conflict') throw E('rev_mismatch', undefined, { current: view(rec, ctx) });
    const r = replaceContent(rec, { buf: v.content }, principal(ctx), rec.item.root_id, ctx);
    return R(200, r, 'MutationResponse');
  });
  route('POST', '/search', (ctx, _p, body) => {
    const rq = parseBody(ctx, body, 'SearchRequest');
    if (ctx.guest) throw E('forbidden', 'Guests cannot search.');
    if (rq.mode !== 'name') needLive(ctx);
    const q = rq.query.toLowerCase(); const hits = [];
    const consider = (item, content) => {
      if (rq.root_ids && !rq.root_ids.includes(item.root_id)) return;
      if (rq.kind && item.kind !== rq.kind) return;
      if (rq.modified_after && item.modified_at < rq.modified_after) return;
      let score = 0, snippet;
      if (item.name.toLowerCase().includes(q)) score = 1;
      else if (rq.mode === 'full_text' && content && /^text\//.test(item.mime || '')) { const t = content.toString('utf8'); const i = t.toLowerCase().indexOf(q); if (i >= 0) { score = 0.6; snippet = t.slice(Math.max(0, i - 20), i + q.length + 40).replace(/\s+/g, ' '); } }
      else if (rq.mode === 'semantic') { const w = q.split(/\s+/).filter(Boolean); const hay = `${item.name} ${content && /^text\//.test(item.mime || '') ? content.toString('utf8') : ''}`.toLowerCase(); const n = w.filter((x) => hay.includes(x)).length; if (n) score = 0.3 + 0.5 * (n / w.length); }
      if (score) hits.push({ item, score, ...(snippet ? { snippet } : {}) });
    };
    if (ctx.live) for (const rec of S.items.values()) { if (isLive(rec) && !S.roots.get(rec.item.root_id)?.policy.protected) consider(view(rec, ctx), rec.content); }
    else for (const f of S.snap.files.values()) for (const s of f.items) consider(snapItem(snapFind(s.id), ctx));
    hits.sort((a, b) => b.score - a.score || a.item.name.localeCompare(b.item.name));
    const limit = Math.min(rq.limit || 50, 200); const off = pageOf(rq.page_token);
    const out = { hits: hits.slice(off, off + limit), index_state: rq.mode === 'name' ? 'ready' : rq.mode === 'full_text' ? 'ready' : 'building', freshness: fresh(ctx) };
    if (off + limit < hits.length) out.next_page_token = pageTok(off + limit);
    return R(200, out, 'SearchResponse');
  });
  route('POST', '/pin', (ctx, _p, body) => {
    needLive(ctx); const rq = parseBody(ctx, body, 'PinRequest'); const rec = getLive(rq.item_id); guardItem(ctx, rec); need(ctx, 'read', rq.item_id);
    const set = new Set(rec.item.pinned_on || []); rq.pinned ? set.add(rq.device_id) : set.delete(rq.device_id);
    rec.item.pinned_on = [...set]; record(rec.item.root_id, 'meta', rec, principal(ctx));
    return R(200, { item_id: rq.item_id, pinned_on: rec.item.pinned_on }, 'PinResponse');
  });
  route('GET', '/roots/([^/]+)/changes', async (ctx, p) => {
    const root = getRoot(ctx, p[0]); need(ctx, 'list');
    const cq = { cursor: ctx.q.get('cursor') ?? undefined };
    if (ctx.q.get('limit') !== null) cq.limit = Number(ctx.q.get('limit')); if (ctx.q.get('wait_s') !== null) cq.wait_s = Number(ctx.q.get('wait_s'));
    const errs = validateType('ChangesQuery', cq); if (errs.length) throw E('invalid_request', errs.slice(0, 3).join('; '));
    const limit = Math.min(cq.limit || 100, 500); const waitS = Math.min(cq.wait_s || 0, 25);
    const log = S.logs.get(root.id);
    const compute = () => {
      const head = ctx.live ? log.seq : seqOf(S.snap.files.get(root.id)?.cursor || root.cursor);
      const headCur = `c1.${root.id}.${String(head).padStart(6, '0')}`;
      const m = /^c1\.(.+)\.(\d+)$/.exec(cq.cursor); const seq = m ? Number(m[2]) : -1;
      if (!m || m[1] !== root.id || seq < log.base || seq > head) return { root_id: root.id, changes: [], cursor: headCur, has_more: false, reset_required: true, freshness: fresh(ctx) };
      if (!ctx.live) return { root_id: root.id, changes: [], cursor: headCur, has_more: false, reset_required: false, freshness: fresh(ctx) };
      let all = log.entries.filter((e) => e.seq > seq);
      if (ctx.guest) all = all.filter((e) => { const r = S.items.get(e.item_id); return r && descends(r, ctx.guest.item_id); });
      const page = all.slice(0, limit); const last = page.length ? page[page.length - 1].seq : seq;
      return { root_id: root.id, changes: page, cursor: `c1.${root.id}.${String(last).padStart(6, '0')}`, has_more: all.length > page.length, reset_required: false, freshness: fresh(ctx) };
    };
    let out = compute();
    if (ctx.live && waitS && !out.changes.length && !out.reset_required) {
      // long poll: never counts as activity, never wakes anything (SPEC 6)
      await new Promise((resolve) => { const w = { root: root.id, resolve }; waiters.add(w); setTimeout(() => { waiters.delete(w); resolve(); }, waitS * 1000); });
      out = compute();
    }
    return R(200, out, 'ChangesResponse');
  });
  route('GET', '/storage', () => { const s = clone(fx('storage')); s.computer_id = cfg.computerId; return R(200, s, 'StorageDiscovery'); });
  route('GET', '/items/([^/]+)/comments', (ctx, p) => {
    needLive(ctx); const rec = getLive(p[0]); guardItem(ctx, rec); need(ctx, 'read', p[0]);
    const list = S.comments.get(p[0]) || []; for (const c of list) { const e = validateType('Comment', c); if (e.length) throw new Error(e.join()); }
    return R(200, list);
  });
  route('POST', '/comments', (ctx, _p, body) => {
    needLive(ctx); const rq = parseBody(ctx, body, 'CommentCreateRequest'); const rec = getLive(rq.item_id); guardItem(ctx, rec); need(ctx, 'comment', rq.item_id);
    audit(ctx, 'comment', rq.item_id, 'allowed');
    const c = { comment_id: `cm_${rid(8)}`, item_id: rq.item_id, author: principal(ctx), body: rq.body, created_at: nowIso() };
    S.comments.set(rq.item_id, [...(S.comments.get(rq.item_id) || []), c]); return R(201, c, 'Comment');
  });
  route('GET', '/replicas', (ctx) => { needLive(ctx); if (ctx.guest) throw E('forbidden'); return R(200, { replicas: S.replicas }, 'ReplicasResponse'); });
  route('POST', '/replicas', (ctx, _p, body) => {
    needLive(ctx); const rq = parseBody(ctx, body, 'CreateReplicaRequest');
    const r = { replica_id: `rp_${rid(6)}`, source_computer_id: rq.source_computer_id, root_id: rq.root_id, host_device_id: rq.host_device_id, mode: rq.mode, state: 'syncing', stored_bytes: 0, key_id: 'fk_1' };
    S.replicas.push(r); return R(201, r, 'Replica');
  });
  route('POST', '/replicas/restore', (ctx, _p, body) => {
    needLive(ctx); const rq = parseBody(ctx, body, 'ReplicaRestoreRequest');
    if (!S.replicas.find((r) => r.replica_id === rq.replica_id)) throw E('not_found', `No replica ${rq.replica_id}.`);
    const rep = clone(fx(rq.replica_id === 'rp_tampered' ? 'replica-restore-tampered' : 'replica-restore-ok')); rep.replica_id = rq.replica_id;
    return R(200, rep, 'ReplicaRestoreReport');
  });

  // ---- cloud-api
  const keyInfo = () => ({ key_id: 'fk_1', created_at: '2026-11-15T14:00:00Z', device_ids: ['dev_iphone16', ...S.envelopes.keys()] });
  const ownerOnly = (ctx) => { if (ctx.guest) throw E('forbidden', 'Owner only.'); };
  cloud('GET', 'computer-files/([^/]+)/endpoints', (ctx, p) => {
    ownerOnly(ctx);
    const c = [{ kind: 'loopback', base_url: `http://127.0.0.1:${cfg.port}/api/v1/files/v2`, priority: 1, max_request_bytes: 5242880 }, { kind: 'relay', base_url: `http://127.0.0.1:${cfg.port}/api/v1/runtime-devices/${p[0]}/proxy`, priority: 10, max_request_bytes: 5242880 }];
    return R(200, { computer_id: p[0], computer_state: ctx.state, wake: S.wakeSetting, candidates: c, host: 'cloud_computer' }, 'EndpointsResponse');
  });
  cloud('POST', 'computer-files/([^/]+)/wake-consent', (ctx, p, body) => {
    const rq = parseBody(ctx, body, 'WakeConsentRequest');
    if (ctx.guest) { const sh = S.shares.get(ctx.guest.share_id); if (!sh?.guest_wake) { audit(ctx, 'wake', undefined, 'denied', 'guest_wake_not_allowed'); throw E('guest_wake_not_allowed'); } audit(ctx, 'wake', undefined, 'allowed'); }
    if (S.wakeSetting === 'never') throw E('wake_denied');
    const before = ctx.state;
    if (before === 'asleep' && ctx.scn === S.scenario) startWake(); else if (before === 'asleep') S.touches += 1; else if (before === 'waking') S.touches += 1; else S.touches += 1;
    const iat = Math.floor(Date.now() / 1000);
    const claims = { v: 1, typ: 'wake', sub: OWNER.id, computer_id: cfg.computerId, reason: rq.reason, device_id: rq.device_id, iat, exp: iat + 120, jti: `jti_${rid(10)}`, kid: KID };
    const out = { ticket: signToken(claims), expires_at: nowIso((iat + 120) * 1000), computer_state: before === 'awake' ? 'awake' : 'waking' };
    if (out.computer_state === 'waking') out.estimated_wake_s = Math.max(1, Math.ceil(cfg.wakeMs / 1000));
    return R(200, out, 'WakeConsentResponse');
  });
  cloud('GET', 'computer-files/([^/]+)/wake-settings', (ctx, p) => { ownerOnly(ctx); return R(200, { computer_id: p[0], wake: S.wakeSetting }, 'WakeSettings'); });
  cloud('PUT', 'computer-files/([^/]+)/wake-settings', (ctx, p, body) => { ownerOnly(ctx); const rq = parseBody(ctx, body, 'UpdateWakeSettingsRequest'); S.wakeSetting = rq.wake; return R(200, { computer_id: p[0], wake: S.wakeSetting }, 'WakeSettings'); });
  cloud('GET', 'computer-files/([^/]+)/storage', (ctx, p) => { const s = clone(fx('storage')); s.computer_id = p[0]; return R(200, s, 'StorageDiscovery'); });
  cloud('POST', 'computer-files/([^/]+)/shares', (ctx, p, body) => {
    ownerOnly(ctx); const rq = parseBody(ctx, body, 'CreateShareRequest');
    const root = S.roots.get(rq.root_id); if (!root) throw E('root_not_found'); const rec = getLive(rq.item_id);
    if (!root.policy.shareable) throw E('forbidden', 'This root is not shareable.');
    const id = `sh_${rid(8)}`; const tok = `${rq.role}${rid(8)}`;
    const s = { share_id: id, computer_id: p[0], root_id: rq.root_id, item_id: rec.item.id, role: rq.role, link_url: `https://ai.allternit.com/s/${tok}`, created_at: nowIso(), created_by: OWNER, passcode_required: !!rq.passcode, guest_wake: rq.guest_wake, guest_count: 0 };
    if (rq.expires_at) s.expires_at = rq.expires_at;
    S.shares.set(id, s); S.links.set(tok, { share_id: id, passcode: rq.passcode || null }); return R(201, { share: s }, 'ShareResponse');
  });
  cloud('GET', 'computer-files/([^/]+)/shares', (ctx) => { ownerOnly(ctx); return R(200, { shares: [...S.shares.values()] }, 'SharesResponse'); });
  cloud('DELETE', 'computer-files/shares/([^/]+)', (ctx, p) => { ownerOnly(ctx); const s = S.shares.get(p[0]); if (!s) throw E('not_found', `No share ${p[0]}.`); s.revoked_at = nowIso(); return R(204); });
  cloud('GET', 'computer-files/shares/([^/]+)/access', (ctx, p) => { ownerOnly(ctx); if (!S.shares.get(p[0])) throw E('not_found', `No share ${p[0]}.`); const rows = S.audit.filter((a) => a.share_id === p[0]); for (const r of rows) { const e = validateType('ShareAccess', r); if (e.length) throw new Error(e.join()); } return R(200, rows); });
  cloud('GET', 'computer-files/token-keys', () => R(200, { keys: [{ kid: KID, alg: 'Ed25519', public_key: pubJwk.x }] }));
  cloud('GET', 'files-keys', () => R(200, keyInfo(), 'FilesKeyInfo'));
  cloud('POST', 'files-keys/requests', (ctx, _p, body) => { parseBody(ctx, body, 'KeyShareRequest'); return R(202); });
  cloud('POST', 'files-keys/grants', (ctx, _p, body) => { const g = parseBody(ctx, body, 'KeyShareGrant'); S.envelopes.set(g.envelope.device_id, g.envelope); return R(204); });
  cloud('GET', 'files-keys/envelopes/([^/]+)', (ctx, p) => { const e = S.envelopes.get(p[0]); if (!e) throw E('not_found', `No envelope for ${p[0]}.`); return R(200, e, 'KeyEnvelope'); });
  const shareFor = (tok) => {
    const l = S.links.get(tok); const s = l && S.shares.get(l.share_id);
    if (!s) throw E('not_found', 'No such share link.');
    if (s.revoked_at) throw E('share_revoked', 'This share link was turned off.');
    if (s.expires_at && Date.parse(s.expires_at) < Date.now()) throw E('share_expired', 'This share link has expired.');
    return { s, l };
  };
  const summaryOf = (s, ctx) => { const sm = { role: s.role, folder_name: S.items.get(s.item_id)?.item.name || 'Shared folder', owner_display_name: OWNER.display_name, can_wake: s.guest_wake, computer_state: ctx.state }; if (s.expires_at) sm.expires_at = s.expires_at; return sm; };
  cloud('GET', 'files-gateway/summary', (ctx) => { const { s } = shareFor(ctx.q.get('link') || ''); return R(200, summaryOf(s, ctx), 'ShareSummary'); });
  cloud('POST', 'files-gateway/redeem', (ctx, _p, body) => {
    const rq = parseBody(ctx, body, 'GuestRedeemRequest'); const { s, l } = shareFor(rq.link_token);
    if (l.passcode) { if (!rq.passcode) throw E('passcode_required', 'This link needs a passcode.'); if (rq.passcode !== l.passcode) throw E('passcode_invalid', 'That passcode is not right.'); }
    const gid = `gst_${rid(6)}`; S.guests.set(gid, { name: rq.guest_name });
    const iat = Math.floor(Date.now() / 1000); const exp = Math.min(iat + 3600, s.expires_at ? Math.floor(Date.parse(s.expires_at) / 1000) : iat + 3600);
    const claims = { v: 1, typ: 'files_guest', share_id: s.share_id, guest_id: gid, owner_user_id: OWNER.id, computer_id: cfg.computerId, root_id: s.root_id, item_id: s.item_id, role: s.role, guest_wake: s.guest_wake, passcode_ok: !!l.passcode, iat, exp, kid: KID };
    audit({ guest: claims }, 'redeem', s.item_id, 'allowed');
    return R(200, { capability_token: signToken(claims), expires_at: nowIso(exp * 1000), guest_id: gid, share: summaryOf(s, ctx) }, 'GuestSessionResponse');
  });
  routes.push({ method: 'POST', re: /^\/api\/v1\/runtime-devices\/([^/]+)\/proxy\/?$/, cloud: true, raw: true, fn: null });

  // ---- stub control (not part of the protocol)
  const stubRoute = (method, p, fn) => routes.push({ method, re: new RegExp(`^/__stub/${p}/?$`), fn, stub: true });
  stubRoute('GET', 'state', () => R(200, { scenario: S.scenario, computer_id: cfg.computerId, wake_setting: S.wakeSetting, activity_touches: S.touches, scenarios: SCENARIOS, port: cfg.port, snapshot_at: S.snap.at }));
  stubRoute('POST', 'scenario', (ctx, _p, body) => { const o = JSON.parse(body.toString() || '{}'); setScenario(o.scenario); return R(200, { scenario: S.scenario }); });
  stubRoute('POST', 'reset', () => { clearTimeout(wakeTimer); seed(); return R(200, { scenario: S.scenario, reset: true }); });
  stubRoute('GET', 'events', () => R(200, { events: S.events }));
  stubRoute('GET', 'errors', () => R(200, { codes: STATUS }));

  // ------------------------------------------------------------ dispatch
  const cors = { 'Access-Control-Allow-Origin': '*', 'Access-Control-Allow-Headers': '*', 'Access-Control-Allow-Methods': 'GET,POST,PUT,PATCH,DELETE,OPTIONS', 'Access-Control-Expose-Headers': 'ETag,Content-Range,Retry-After,X-Files-Served-By' };
  function readBody(req, limit) {
    return new Promise((resolve, reject) => {
      const parts = []; let n = 0; let over = false;
      req.on('data', (c) => { n += c.length; if (n > limit) { over = true; return; } parts.push(c); });
      req.on('end', () => (over ? reject(E('payload_too_large', `Body is over ${limit} bytes.`)) : resolve(Buffer.concat(parts))));
      req.on('error', reject);
    });
  }
  function sendErr(res, err) {
    const e = err instanceof StubError ? err : (console.error(err), new StubError('internal', String(err.message || err)));
    const body = errBody(e.code, e.message, e.extra);
    const bad = validateType('ErrorBody', body);
    if (bad.length) console.error('stub produced an invalid ErrorBody', bad);
    const h = { ...cors, 'Content-Type': 'application/json' };
    if (body.retry_after_s) h['Retry-After'] = String(body.retry_after_s);
    res.writeHead(STATUS[e.code] || 500, h); res.end(JSON.stringify(body));
  }
  async function send(ctx, res, r) {
    const h = { ...cors, ...(r.headers || {}) };
    if (r.bytes) {
      res.writeHead(r.status, { ...h, 'Content-Length': r.bytes.length });
      if (ctx.scn === 'slow') { for (let o = 0; o < r.bytes.length; o += 65536) { res.write(r.bytes.subarray(o, o + 65536)); await sleep((65536 / cfg.slowBps) * 1000); } res.end(); } else res.end(r.bytes);
      return;
    }
    if (r.body === undefined) { res.writeHead(r.status, h); return res.end(); }
    if (r.type) {
      const errs = validateType(r.type, r.body);
      if (errs.length) { console.error(`stub produced an invalid ${r.type}:`, errs.slice(0, 5)); throw new Error(`response is not a valid ${r.type}: ${errs.slice(0, 3).join('; ')}`); }
    }
    res.writeHead(r.status, { ...h, 'Content-Type': 'application/json' }); res.end(JSON.stringify(r.body));
  }
  const errorCodes = Object.keys(STATUS);
  async function handle(req, res) {
    try {
      const url = new URL(req.url, 'http://stub');
      if (req.method === 'OPTIONS') { res.writeHead(204, cors); return res.end(); }
      const rt = routes.find((r) => r.method === req.method && r.re.test(url.pathname));
      if (!rt) throw E(routes.some((r) => r.re.test(url.pathname)) ? 'unsupported' : 'not_found', `${req.method} ${url.pathname} is not a Files stub route.`);
      const limit = rt.body || 1048576;
      const body = ['POST', 'PUT', 'PATCH'].includes(req.method) ? await readBody(req, limit) : Buffer.alloc(0);
      if (rt.stub) { const r = rt.fn({}, [], body); return await send({ scn: 'awake' }, res, r); }
      if (rt.raw) return await relay(req, res, body, url);
      const ctx = await makeCtx(req, url);
      if (ctx.scn === 'error') {
        let code = ctx.h['x-files-stub-error'];
        if (!code || code === 'next') code = errorCodes[S.errCycle++ % errorCodes.length];
        if (!STATUS[code]) throw E('invalid_request', `X-Files-Stub-Error must be an ErrorCode (see GET /__stub/errors).`);
        throw new StubError(code, undefined, code === 'rev_mismatch' ? { current: view([...S.items.values()].find((r) => r.item.kind === 'file')) } : code === 'chunk_missing' ? { missing_chunks: [blake3hex(Buffer.from('missing'))] } : {});
      }
      if (ctx.scn === 'slow' && body.length) await sleep((body.length / cfg.slowBps) * 1000);
      const m = rt.re.exec(url.pathname);
      const r = await rt.fn(ctx, m.slice(1), body);
      return await send(ctx, res, r);
    } catch (e) { if (!res.headersSent) sendErr(res, e); else res.destroy(); }
  }
  async function relay(req, res, body, url) {
    const rq = JSON.parse(body.toString() || '{}');
    if (typeof rq.method !== 'string' || typeof rq.path !== 'string') throw E('invalid_request', 'RelayRequest needs method and path.');
    const payload = rq.body ? ((rq.body_encoding ?? rq.bodyEncoding) === 'base64' ? Buffer.from(rq.body, 'base64') : Buffer.from(rq.body)) : null;
    const hdrs = { ...(rq.headers || {}), 'x-allternit-relayed': '1' };
    for (const k of ['x-files-stub-scenario', 'x-files-stub-error']) if (req.headers[k]) hdrs[k] = req.headers[k];
    if (payload) hdrs['content-length'] = payload.length;
    const out = await new Promise((resolve, reject) => {
      const r2 = http.request({ host: '127.0.0.1', port: server.address().port, path: rq.path, method: rq.method, headers: hdrs }, (rs) => { const ps = []; rs.on('data', (c) => ps.push(c)); rs.on('end', () => resolve({ status: rs.statusCode, headers: rs.headers, buf: Buffer.concat(ps) })); });
      r2.on('error', reject); if (payload) r2.write(payload); r2.end();
    });
    // SPEC 2 (Relay): cloud-api streams the runtime response as-is (status, allowed headers, raw bytes); no envelope.
    const pass = ['content-type', 'cache-control', 'content-disposition', 'etag', 'last-modified', 'x-request-id', 'content-range', 'accept-ranges', 'retry-after'];
    res.writeHead(out.status, { ...cors, ...Object.fromEntries(Object.entries(out.headers).filter(([k]) => pass.includes(k))) });
    res.end(out.buf);
  }

  // ------------------------------------------------------------ WebSocket change feed (minimal RFC 6455)
  const frame = (obj) => { const p = Buffer.from(JSON.stringify(obj)); const h = p.length < 126 ? Buffer.from([0x81, p.length]) : p.length < 65536 ? Buffer.from([0x81, 126, p.length >> 8, p.length & 255]) : (() => { const b = Buffer.alloc(10); b[0] = 0x81; b[1] = 127; b.writeBigUInt64BE(BigInt(p.length), 2); return b; })(); return Buffer.concat([h, p]); };
  function upgrade(req, socket) {
    const url = new URL(req.url, 'http://stub');
    if (!new RegExp(`^${V2}/subscribe$`).test(url.pathname) || !req.headers['sec-websocket-key']) { socket.end('HTTP/1.1 404 Not Found\r\n\r\n'); return; }
    socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${crypto.createHash('sha1').update(req.headers['sec-websocket-key'] + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest('base64')}\r\n\r\n`);
    sockets.add(socket); socket.on('close', () => sockets.delete(socket));
    const scn = req.headers['x-files-stub-scenario'] || S.scenario;
    const subs = new Map(); let hb;
    const out = (o) => { const e = validateType('FeedMessage', o); if (e.length) console.error('invalid FeedMessage', e); if (!socket.destroyed) socket.write(frame(o)); };
    const close = (code = 1000) => { clearInterval(hb); wsClients.delete(client); if (!socket.destroyed) socket.end(Buffer.from([0x88, 2, code >> 8, code & 255])); };
    const pump = (rootId) => {
      const cur = subs.get(rootId); if (cur === undefined) return;
      const log = S.logs.get(rootId); if (!log) return;
      const seq = seqOf(cur);
      if (seq < log.base || seq > log.seq) { out({ type: 'reset', root_id: rootId, reason: 'cursor_expired' }); subs.delete(rootId); return; }
      const ents = log.entries.filter((e) => e.seq > seq); if (!ents.length) return;
      const c = `c1.${rootId}.${String(log.seq).padStart(6, '0')}`; subs.set(rootId, c);
      out({ type: 'changes', root_id: rootId, changes: ents, cursor: c });
    };
    const client = { onChange: (rootId) => { if (stateOf(S.scenario) === 'awake') pump(rootId); }, onState: (st) => { const f = { type: 'state', computer_state: st }; if (st !== 'awake') f.snapshot_at = S.snap.at; out(f); if (st === 'asleep') close(); } };
    wsClients.add(client);
    let buf = Buffer.alloc(0); let got = false;
    socket.on('data', (d) => {
      buf = Buffer.concat([buf, d]);
      while (buf.length >= 2) {
        let len = buf[1] & 127; let off = 2;
        if (len === 126) { if (buf.length < 4) return; len = buf.readUInt16BE(2); off = 4; } else if (len === 127) { if (buf.length < 10) return; len = Number(buf.readBigUInt64BE(2)); off = 10; }
        const masked = !!(buf[1] & 128); if (buf.length < off + (masked ? 4 : 0) + len) return;
        const mask = masked ? buf.subarray(off, off + 4) : null; off += masked ? 4 : 0;
        const payload = Buffer.from(buf.subarray(off, off + len)); if (mask) for (let i = 0; i < len; i++) payload[i] ^= mask[i % 4];
        const op = buf[0] & 15; buf = buf.subarray(off + len);
        if (op === 8) return close();
        if (op === 9) { socket.write(Buffer.concat([Buffer.from([0x8a, payload.length]), payload])); continue; }
        if (op !== 1 || got) continue;
        let msg; try { msg = JSON.parse(payload.toString()); } catch { return close(1008); }
        if (validateType('SubscribeMessage', msg).length) return close(1008);
        got = true;
        if (scn === 'asleep' || S.scenario === 'asleep') { out({ type: 'state', computer_state: 'asleep', snapshot_at: S.snap.at }); return close(); }
        for (const r of msg.roots) { subs.set(r.root_id, r.cursor); if (!S.logs.has(r.root_id)) { out({ type: 'reset', root_id: r.root_id, reason: 'root_removed' }); subs.delete(r.root_id); } else pump(r.root_id); }
        hb = setInterval(() => out({ type: 'heartbeat', at: nowIso() }), cfg.heartbeatMs);
      }
    });
    socket.on('close', () => { clearInterval(hb); wsClients.delete(client); });
    socket.on('error', () => {});
  }

  // ------------------------------------------------------------ lifecycle
  seed();
  const server = http.createServer((req, res) => { handle(req, res); });
  server.on('upgrade', upgrade);
  server.on('connection', (s) => { sockets.add(s); s.on('close', () => sockets.delete(s)); });
  return {
    server, cfg,
    listen: () => new Promise((resolve) => server.listen(cfg.port, '127.0.0.1', () => { cfg.port = server.address().port; resolve(cfg.port); })),
    close: () => new Promise((resolve) => { clearTimeout(wakeTimer); for (const w of waiters) w.resolve(); for (const s of sockets) s.destroy(); server.close(() => resolve()); }),
    state: () => S,
  };
}
