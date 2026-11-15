// Scripted self-test: boots the stub on an ephemeral port, runs a request sequence over HTTP,
// and validates every JSON response body against the contract schemas.
import http from 'node:http';
import crypto from 'node:crypto';
import { createStub, ERROR_STATUS } from './stub.mjs';
import { blake3hex } from './blake3.mjs';
import { validateType, defs } from '../../contracts/files-drive/schema-lib.mjs';

export async function runSelfTest() {
  const stub = createStub({ port: 0, wakeMs: 400, slowBps: 1048576, quiet: true });
  const port = await stub.listen();
  const base = `http://127.0.0.1:${port}`;
  const V = '/api/v1/files/v2';
  let n = 0, bad = 0;
  const check = (ok, msg) => { n++; if (!ok) { bad++; console.error(`FAIL ${msg}`); } };

  async function call(method, p, { body, raw, headers = {}, type, status = 200, text = false } = {}) {
    const h = { ...headers };
    let payload = raw;
    if (body !== undefined) { payload = JSON.stringify(body); h['content-type'] = 'application/json'; }
    const r = await fetch(base + p, { method, headers: h, body: payload });
    const buf = Buffer.from(await r.arrayBuffer());
    let json;
    if (!text && buf.length && /json/.test(r.headers.get('content-type') || '')) json = JSON.parse(buf.toString());
    const label = `${method} ${p.split('?')[0]}`;
    check(r.status === status, `${label}: expected ${status}, got ${r.status} ${buf.toString().slice(0, 200)}`);
    if (type && json !== undefined && r.status === status) { const e = validateType(type, json); check(!e.length, `${label}: invalid ${type}: ${e.slice(0, 3).join('; ')}`); }
    return { r, json, buf };
  }
  const err = (code, p, o = {}) => call(o.method || 'GET', p, { ...o, status: ERROR_STATUS[code], type: 'ErrorBody' }).then((x) => { check(x.json?.code === code, `${p}: expected ${code}, got ${x.json?.code}`); return x; });
  const scn = (s, extra = {}) => ({ 'x-files-stub-scenario': s, ...extra });

  // ---------------------------------------------------------------- awake
  await call('GET', `${V}/ping`, { type: 'PingResponse' });
  await call('GET', `${V}/whoami`, { type: 'WhoAmIResponse' });
  const roots = (await call('GET', `${V}/roots`, { type: 'RootsResponse' })).json;
  check(roots.freshness.served_by === 'live', 'roots served live');
  const home = roots.roots.find((r) => r.id === 'rt_home');
  const list = (await call('GET', `${V}/roots/rt_home/list?parent_id=${home.root_item_id}`, { type: 'ListResponse' })).json;
  const lease = list.items.find((i) => i.name === 'Lease.pdf');
  check(!!lease, 'Lease.pdf listed');
  await call('GET', `${V}/roots/rt_home/list?limit=1&parent_id=${home.root_item_id}`, { type: 'ListResponse' }).then((x) => check(!!x.json.next_page_token, 'list paging token'));
  await call('GET', `${V}/roots/rt_home/list`, { status: 400, type: 'ErrorBody' });
  await call('GET', `${V}/items/${lease.id}`, { type: 'StatResponse' });
  const touches0 = stub.state().touches;
  const c1 = await call('GET', `${V}/items/${lease.id}/content`, { text: true });
  check(blake3hex(c1.buf) === (await call('GET', `${V}/items/${lease.id}`, { type: 'StatResponse' })).json.item.content_hash, 'content hash matches item');
  const etag = c1.r.headers.get('etag');
  await call('GET', `${V}/items/${lease.id}/content`, { headers: { 'if-none-match': etag }, status: 304, text: true });
  const rg = await call('GET', `${V}/items/${lease.id}/content`, { headers: { range: 'bytes=0-9' }, status: 206, text: true });
  check(rg.buf.length === 10 && rg.r.headers.get('content-range') === `bytes 0-9/${c1.buf.length}`, 'range 206');
  await call('GET', `${V}/items/${lease.id}/manifest`, { type: 'Manifest' });
  const man = (await call('GET', `${V}/items/${lease.id}/manifest`, { type: 'Manifest' })).json;
  const chunk0 = await call('GET', `${V}/chunks/${man.chunks[0].hash}`, { text: true });
  check(blake3hex(chunk0.buf) === man.chunks[0].hash, 'GET chunk matches its hash');
  await call('POST', `${V}/chunks/have-need`, { body: { hashes: [man.chunks[0].hash, 'f'.repeat(64)] }, type: 'HaveNeedResponse' }).then((x) => check(x.json.have.length === 1 && x.json.need.length === 1, 'have/need split'));
  check(stub.state().touches === touches0, 'browsing never touches activity');

  // small write, replace, conflict, fail
  const w = await call('PUT', `${V}/roots/rt_home/content?parent_id=${home.root_item_id}&name=small.txt&on_conflict=fail`, { raw: 'hello', type: 'MutationResponse' });
  const small = w.json.item;
  const w2 = await call('PUT', `${V}/roots/rt_home/content?parent_id=${home.root_item_id}&name=small.txt&base_rev=${small.rev}&on_conflict=keep_both`, { raw: 'hello again', type: 'MutationResponse' });
  check(w2.json.item.rev !== small.rev && !w2.json.conflict, 'replace at current base_rev');
  const w3 = await call('PUT', `${V}/roots/rt_home/content?parent_id=${home.root_item_id}&name=small.txt&base_rev=${small.rev}&on_conflict=keep_both`, { raw: 'stale edit', type: 'MutationResponse', headers: { 'x-allternit-device-id': 'dev_iphone16', 'x-allternit-device-name': 'iPhone' } });
  check(w3.json.conflict?.kind === 'keep_both' && /^small \(conflict iPhone \d{4}-\d\d-\d\d \d{4}\)\.txt$/.test(w3.json.conflict.conflict_name), `keep-both conflict copy named per SPEC (${w3.json.conflict?.conflict_name})`);
  await err('rev_mismatch', `${V}/roots/rt_home/content?parent_id=${home.root_item_id}&name=small.txt&base_rev=${small.rev}&on_conflict=fail`, { method: 'PUT', raw: 'x' });
  await err('already_exists', `${V}/roots/rt_home/content?parent_id=${home.root_item_id}&name=small.txt&on_conflict=fail`, { method: 'PUT', raw: 'x' });
  await err('payload_too_large', `${V}/roots/rt_home/content?parent_id=${home.root_item_id}&name=big.bin`, { method: 'PUT', raw: Buffer.alloc(4 * 1048576 + 1) });
  await err('invalid_name', `${V}/roots/rt_home/content?parent_id=${home.root_item_id}&name=a%2Fb`, { method: 'PUT', raw: 'x' });
  await call('GET', `${V}/items/${small.id}/versions`, { type: 'VersionsResponse' }).then(async (x) => {
    check(x.json.versions.length === 1, 'one old version kept');
    const cur = (await call('GET', `${V}/items/${small.id}`, { type: 'StatResponse' })).json.item;
    await call('POST', `${V}/items/${small.id}/versions/restore`, { body: { version_id: x.json.versions[0].version_id, base_rev: cur.rev }, type: 'MutationResponse' });
  });

  // folders: mkdir, move, copy, delete, trash, restore
  const dir = (await call('POST', `${V}/mkdir`, { body: { parent_id: home.root_item_id, name: 'Projects', on_conflict: 'fail' }, status: 201, type: 'MutationResponse' })).json.item;
  const mv = await call('POST', `${V}/move`, { body: { item_id: small.id, base_rev: (await call('GET', `${V}/items/${small.id}`, { type: 'StatResponse' })).json.item.rev, new_parent_id: dir.id, on_conflict: 'fail' }, type: 'MutationResponse' });
  await err('rev_mismatch', `${V}/move`, { method: 'POST', body: { item_id: small.id, base_rev: small.rev, new_name: 'z.txt', on_conflict: 'fail' } });
  await call('POST', `${V}/move`, { body: { item_id: small.id, new_name: 'z.txt', on_conflict: 'fail' }, status: 400, type: 'ErrorBody' });
  const cp = await call('POST', `${V}/copy`, { body: { item_id: small.id, dest_parent_id: home.root_item_id, on_conflict: 'keep_both' }, type: 'CopyResponse' });
  check(!!cp.json.item, 'copy returns item');
  const job = await call('POST', `${V}/copy`, { body: { item_id: small.id, dest_parent_id: home.root_item_id, dest_computer_id: 'cmp_other', on_conflict: 'keep_both' }, status: 202, type: 'CopyResponse' });
  await call('GET', `${V}/jobs/${job.json.job.job_id}`, { type: 'JobRef' });
  const del = await call('POST', `${V}/delete`, { body: { item_id: cp.json.item.id, permanent: false }, type: 'DeleteResponse' });
  const tr = await call('GET', `${V}/roots/rt_home/trash`, { type: 'TrashListResponse' });
  check(tr.json.entries.some((e) => e.trash_id === del.json.trashed.trash_id), 'trashed entry listed');
  await call('POST', `${V}/restore`, { body: { trash_id: del.json.trashed.trash_id, on_conflict: 'keep_both' }, type: 'MutationResponse' });
  await call('POST', `${V}/delete`, { body: { item_id: dir.id, permanent: true }, type: 'DeleteResponse' });
  await call('POST', `${V}/pin`, { body: { item_id: lease.id, device_id: 'dev_iphone16', pinned: true }, type: 'PinResponse' }).then((x) => check(x.json.pinned_on.includes('dev_iphone16'), 'pin'));
  for (const mode of ['name', 'full_text', 'semantic']) await call('POST', `${V}/search`, { body: { query: 'lease', mode }, type: 'SearchResponse' }).then((x) => check(x.json.hits.length > 0, `search ${mode} finds a hit`));
  await call('POST', `${V}/comments`, { body: { item_id: lease.id, body: 'looks right' }, status: 201, type: 'Comment' });
  await call('GET', `${V}/items/${lease.id}/comments`).then((x) => check(x.json.length === 1, 'comment listed'));
  await call('GET', `${V}/storage`, { type: 'StorageDiscovery' });
  await call('GET', `${V}/replicas`, { type: 'ReplicasResponse' });
  await call('POST', `${V}/replicas`, { body: { source_computer_id: 'cmp_cloud_4gb', root_id: 'rt_home', host_device_id: 'dev_macbook1', mode: 'full' }, status: 201, type: 'Replica' });
  await call('POST', `${V}/replicas/restore`, { body: { replica_id: 'rp_1', target_computer_id: 'cmp_cloud_4gb' }, type: 'ReplicaRestoreReport' });
  await err('protected_root_locked', `${V}/roots/rt_memory/list?parent_id=it_mem00000`);
  await call('GET', `${V}/roots/rt_memory/list?parent_id=it_mem00000`, { headers: { 'x-allternit-unlock': '1' }, type: 'ListResponse' });
  await err('scope_violation', `${V}/roots`, { method: 'POST', body: { name: 'ssh', local_path: '/etc', policy: roots.roots[0].policy } });
  await err('root_not_found', `${V}/roots/rt_nope/list?parent_id=it_home0000`);
  await err('not_found', `${V}/items/it_doesnotexist`);
  await err('invalid_request', `${V}/mkdir`, { method: 'POST', body: { parent_id: home.root_item_id, name: 'x', on_conflict: 'fail', extra: 1 } });

  // ---------------------------------------------------------------- chunked upload, 9 MiB, 3 chunks
  const MiB = 1048576;
  const file = crypto.randomBytes(9 * MiB);
  const parts = [[0, 4 * MiB], [4 * MiB, 4 * MiB], [8 * MiB, MiB]].map(([o, s]) => ({ off: o, buf: file.subarray(o, o + s) }));
  const refs = parts.map((p) => ({ hash: blake3hex(p.buf), offset: p.off, size: p.buf.length }));
  const chunker = { algo: 'fastcdc-2020', hash: 'blake3', min_bytes: 262144, avg_bytes: MiB, max_bytes: 4 * MiB };
  const fileHash = blake3hex(file);
  const hn = await call('POST', `${V}/chunks/have-need`, { body: { hashes: refs.map((r) => r.hash) }, type: 'HaveNeedResponse' });
  check(hn.json.need.length === 3, 'all three chunks needed');
  const up = (await call('POST', `${V}/uploads`, { body: { root_id: 'rt_home', parent_id: home.root_item_id, name: 'big.bin', size: file.length, content_hash: fileHash, chunker, chunks: refs, total_chunks: 3, on_conflict: 'fail' }, status: 201, type: 'UploadStatus' })).json;
  check(up.need.length === 3, 'session needs 3');
  await call('POST', `${V}/uploads`, { body: { root_id: 'rt_home', parent_id: home.root_item_id, name: 'x', size: 5 * MiB, content_hash: fileHash, chunker, chunks: [{ hash: refs[0].hash, offset: 0, size: 5 * MiB }], total_chunks: 1, on_conflict: 'fail' }, status: 400, type: 'ErrorBody' });
  await err('chunk_missing', `${V}/uploads/${up.session_id}/commit`, { method: 'POST' }).then((x) => check(x.json.missing_chunks?.length === 3, 'commit lists missing chunks'));
  await err('chunk_hash_mismatch', `${V}/uploads/${up.session_id}/chunks/${refs[0].hash}`, { method: 'PUT', raw: parts[1].buf });
  await err('payload_too_large', `${V}/uploads/${up.session_id}/chunks/${refs[0].hash}`, { method: 'PUT', raw: Buffer.alloc(4 * MiB + 1) });
  const put0 = await call('PUT', `${V}/uploads/${up.session_id}/chunks/${refs[0].hash}`, { raw: parts[0].buf, type: 'ChunkPutResponse' });
  check(put0.json.stored && put0.json.remaining === 2, 'chunk 0 stored, 2 remaining');
  const st = (await call('GET', `${V}/uploads/${up.session_id}`, { type: 'UploadStatus' })).json;
  check(st.need.length === 2 && st.received_bytes === 4 * MiB, 'resume shows 2 chunks still needed');
  for (const i of [1, 2]) await call('PUT', `${V}/uploads/${up.session_id}/chunks/${refs[i].hash}`, { raw: parts[i].buf, type: 'ChunkPutResponse' });
  const before = (await call('GET', `${V}/roots/rt_home/changes?cursor=${home.cursor}`, { type: 'ChangesResponse' })).json.cursor;
  const commit = (await call('POST', `${V}/uploads/${up.session_id}/commit`, { type: 'UploadCommitResponse' })).json;
  check(commit.item.content_hash === fileHash && commit.item.size === file.length, 'committed item has the whole-file hash');
  const back = await call('GET', `${V}/items/${commit.item.id}/content`, { text: true });
  check(back.buf.equals(file), 'read-back bytes equal upload');
  const lst = (await call('GET', `${V}/roots/rt_home/list?parent_id=${home.root_item_id}`, { type: 'ListResponse' })).json;
  check(lst.items.some((i) => i.id === commit.item.id), 'new revision is listed');
  const feed = (await call('GET', `${V}/roots/rt_home/changes?cursor=${before}`, { type: 'ChangesResponse' })).json;
  check(feed.changes.some((c) => c.item_id === commit.item.id && c.op === 'created'), 'change feed shows the upload');
  const feed2 = (await call('GET', `${V}/roots/rt_home/changes?cursor=${feed.cursor}`, { type: 'ChangesResponse' })).json;
  check(feed2.changes.length === 0 && feed2.cursor === feed.cursor, 'cursor is stable once caught up');
  await call('GET', `${V}/roots/rt_home/changes?cursor=c1.rt_home.000001`, { type: 'ChangesResponse' }).then((x) => check(x.json.reset_required, 'old cursor -> reset_required'));
  // replace the file through a second upload (new revision)
  const up2 = (await call('POST', `${V}/uploads`, { body: { root_id: 'rt_home', parent_id: home.root_item_id, name: 'big.bin', item_id: commit.item.id, base_rev: commit.item.rev, size: 3, content_hash: blake3hex(Buffer.from('new')), chunker, chunks: [{ hash: blake3hex(Buffer.from('new')), offset: 0, size: 3 }], total_chunks: 1, on_conflict: 'keep_both' }, status: 201, type: 'UploadStatus' })).json;
  await call('PUT', `${V}/uploads/${up2.session_id}/chunks/${blake3hex(Buffer.from('new'))}`, { raw: 'new', type: 'ChunkPutResponse' });
  const c2 = (await call('POST', `${V}/uploads/${up2.session_id}/commit`, { type: 'UploadCommitResponse' })).json;
  check(c2.item.id === commit.item.id && c2.item.rev !== commit.item.rev, 'upload replaces at base_rev, new rev');
  await call('DELETE', `${V}/uploads/${up2.session_id}`, { status: 204 });
  await err('upload_session_not_found', `${V}/uploads/up_nope`);

  // long poll
  const lp0 = (await call('GET', `${V}/roots/rt_inbox/changes?cursor=c1.rt_inbox.000019`, { type: 'ChangesResponse' })).json.cursor;
  const t0 = Date.now();
  const pending = call('GET', `${V}/roots/rt_inbox/changes?cursor=${lp0}&wait_s=5`, { type: 'ChangesResponse' });
  await new Promise((r) => setTimeout(r, 200));
  await call('PUT', `${V}/roots/rt_inbox/content?parent_id=it_inbox000&name=drop.txt`, { raw: 'dropped', type: 'MutationResponse' });
  const lp = (await pending).json;
  check(lp.changes.length === 1 && Date.now() - t0 < 4000, 'long poll returns on change');
  check(stub.state().events.some((e) => e.type === 'files.inbox.dropped'), 'inbox drop event emitted');

  // websocket feed
  const ws = await wsConnect(port, `${V}/subscribe`);
  ws.send({ type: 'subscribe', roots: [{ root_id: 'rt_inbox', cursor: lp.cursor }] });
  await new Promise((r) => setTimeout(r, 100));
  await call('PUT', `${V}/roots/rt_inbox/content?parent_id=it_inbox000&name=drop2.txt`, { raw: 'dropped 2', type: 'MutationResponse' });
  await new Promise((r) => setTimeout(r, 200));
  const frames = ws.frames();
  check(frames.some((f) => f.type === 'changes' && f.changes.length === 1), 'websocket delivers a changes frame');
  for (const f of frames) check(!validateType('FeedMessage', f).length, 'websocket frame validates');
  ws.close();

  // cloud routes
  await call('GET', `/api/v1/computer-files/cmp_cloud_4gb/endpoints`, { type: 'EndpointsResponse' });
  await call('GET', `/api/v1/computer-files/cmp_cloud_4gb/wake-settings`, { type: 'WakeSettings' });
  await call('GET', `/api/v1/computer-files/cmp_cloud_4gb/storage`, { type: 'StorageDiscovery' });
  const tk = (await call('GET', `/api/v1/computer-files/token-keys`)).json;
  check(tk.keys?.[0]?.alg === 'Ed25519', 'token-keys lists an Ed25519 key');
  await call('GET', `/api/v1/files-keys`, { type: 'FilesKeyInfo' });
  await call('POST', `/api/v1/files-keys/requests`, { body: { device_id: 'dev_pixel900', device_pub: 'cHVi', device_name: 'Pixel', confirm_code: '123456' }, status: 202 });
  await call('GET', `/api/v1/files-keys/envelopes/dev_pixel900`, { type: 'KeyEnvelope' });
  const rel = await call('POST', `/api/v1/runtime-devices/cmp_cloud_4gb/proxy`, { body: { method: 'GET', path: `${V}/ping`, headers: {}, body: null, body_encoding: 'utf8' } });
  check(rel.json.status === 200 && !validateType('PingResponse', JSON.parse(rel.json.body)).length, 'relay proxy wraps a runtime response');

  // ---------------------------------------------------------------- asleep + wake consent
  const A = scn('asleep');
  const aRoots = (await call('GET', `${V}/roots`, { headers: A, type: 'RootsResponse' })).json;
  check(aRoots.freshness.served_by === 'snapshot' && aRoots.freshness.computer_state === 'asleep' && aRoots.freshness.snapshot_at, 'asleep roots from snapshot');
  check(!aRoots.roots.some((r) => r.policy.protected || r.path_hint), 'snapshot has no protected roots or path hints');
  const aList = (await call('GET', `${V}/roots/rt_home/list?parent_id=${home.root_item_id}`, { headers: A, type: 'ListResponse' })).json;
  check(aList.freshness.served_by === 'snapshot' && aList.items.some((i) => i.name === 'Lease.pdf'), 'asleep list shows snapshot');
  await call('GET', `${V}/items/${lease.id}`, { headers: A, type: 'StatResponse' });
  await call('GET', `${V}/roots/rt_home/changes?cursor=${home.cursor}`, { headers: A, type: 'ChangesResponse' });
  await call('POST', `${V}/search`, { headers: A, body: { query: 'lease', mode: 'name' }, type: 'SearchResponse' });
  const wc = await err('wake_consent_required', `${V}/items/${lease.id}/content`, { headers: A });
  check(wc.json.wake?.consent_path === '/api/v1/computer-files/cmp_cloud_4gb/wake-consent' && wc.json.computer_state === 'asleep', 'WakeHint in 409');
  await err('wake_consent_required', `${V}/roots/rt_home/content?parent_id=${home.root_item_id}&name=a.txt`, { method: 'PUT', headers: A, raw: 'x' });
  await err('wake_consent_required', `${V}/uploads`, { method: 'POST', headers: A, body: { root_id: 'rt_home', parent_id: home.root_item_id, name: 'x', size: 0, content_hash: 'a'.repeat(64), chunker, chunks: [], total_chunks: 0, on_conflict: 'fail' } });
  await err('wake_consent_required', `${V}/search`, { method: 'POST', headers: A, body: { query: 'lease', mode: 'full_text' } });
  // real wake: flip the stub's own state
  await call('POST', `/__stub/scenario`, { body: { scenario: 'asleep' } });
  const touches1 = stub.state().touches;
  const consent = (await call('POST', `/api/v1/computer-files/cmp_cloud_4gb/wake-consent`, { body: { reason: 'open_file', item_id: lease.id, device_id: 'dev_iphone16' }, type: 'WakeConsentResponse' })).json;
  check(consent.computer_state === 'waking' && stub.state().touches === touches1 + 1, 'consent wakes and records exactly one touch');
  await err('waking', `${V}/items/${lease.id}/content`).then((x) => check(x.json.retry_after_s > 0, 'waking has retry_after_s'));
  const waited = await call('GET', `${V}/items/${lease.id}/content`, { headers: { 'x-allternit-wake-ticket': consent.ticket }, text: true });
  check(waited.r.status === 200 && waited.buf.length === lease.size, 'ticket request waits for the wake and succeeds');
  await new Promise((r) => setTimeout(r, 600));
  check((await call('GET', `/__stub/state`)).json.scenario === 'awake', 'stub flipped waking -> awake');
  await call('GET', `${V}/ping`, { type: 'PingResponse' }).then((x) => check(x.json.computer_state === 'awake', 'ping awake after wake'));
  await call('PUT', `/api/v1/computer-files/cmp_cloud_4gb/wake-settings`, { body: { wake: 'never' }, type: 'WakeSettings' });
  await err('wake_denied', `/api/v1/computer-files/cmp_cloud_4gb/wake-consent`, { method: 'POST', body: { reason: 'open_file', device_id: 'dev_iphone16' } });
  await err('wake_denied', `${V}/items/${lease.id}/content`, { headers: A });
  await call('PUT', `/api/v1/computer-files/cmp_cloud_4gb/wake-settings`, { body: { wake: 'ask' }, type: 'WakeSettings' });

  // waking scenario
  await err('waking', `${V}/items/${lease.id}/content`, { headers: scn('waking') });
  await call('GET', `${V}/roots`, { headers: scn('waking'), type: 'RootsResponse' }).then((x) => check(x.json.freshness.computer_state === 'waking', 'waking roots say waking'));

  // conflict scenario: even a write at the current base_rev becomes a conflict copy
  const cur = (await call('GET', `${V}/items/${lease.id}`, { type: 'StatResponse' })).json.item;
  const cf = await call('PUT', `${V}/roots/rt_home/content?parent_id=${home.root_item_id}&name=Lease.pdf&base_rev=${cur.rev}&on_conflict=keep_both`, { headers: scn('conflict'), raw: 'forced', type: 'MutationResponse' });
  check(!!cf.json.conflict, 'conflict scenario forces keep-both');
  check(stub.state().events.some((e) => e.type === 'files.conflict'), 'files.conflict event emitted');

  // slow scenario: 1 MiB/s, 2 MiB chunk takes about 2 s each way
  const s0 = Date.now();
  await call('GET', `${V}/chunks/${man.chunks[0].hash}`, { headers: scn('slow'), text: true });
  const dt = Date.now() - s0;
  check(dt > 300, `slow scenario throttles chunk transfer (${dt} ms)`);

  // ---------------------------------------------------------------- guests
  const redeem = async (link, extra = {}, status = 200) => call('POST', `/api/v1/files-gateway/redeem`, { body: { link_token: link, ...extra }, status, ...(status === 200 ? { type: 'GuestSessionResponse' } : { type: 'ErrorBody' }) });
  await call('GET', `/api/v1/files-gateway/summary?link=view_downloadx9`, { type: 'ShareSummary' });
  await redeem('view_downloadx9', {}, 401).then((x) => check(x.json.code === 'passcode_required', 'passcode required'));
  await redeem('view_downloadx9', { passcode: '0000' }, 401).then((x) => check(x.json.code === 'passcode_invalid', 'passcode invalid'));
  await redeem('expired0stub', {}, 410).then((x) => check(x.json.code === 'share_expired', 'share expired'));
  await redeem('revoked0stub', {}, 410).then((x) => check(x.json.code === 'share_revoked', 'share revoked'));
  const viewer = (await redeem('view_downloadx9', { passcode: '4417', guest_name: 'Dana' })).json;
  const G = { authorization: `Files-Guest ${viewer.capability_token}` };
  const GV = '/api/v1/files-gateway/v2';
  await call('GET', `${GV}/roots`, { headers: G, type: 'RootsResponse' }).then((x) => check(x.json.roots.length === 1 && !x.json.roots[0].path_hint, 'guest sees one root, no path hint'));
  const gl = (await call('GET', `${GV}/roots/rt_shared/list?parent_id=it_shr000001`, { headers: G, type: 'ListResponse' })).json;
  check(gl.items.every((i) => !i.permissions.write), 'view_download items are not writable');
  await call('GET', `${GV}/items/${gl.items[0].id}/content`, { headers: G, text: true });
  await err('scope_violation', `${GV}/items/${lease.id}`, { headers: G });
  await err('forbidden', `${GV}/roots/rt_shared/content?parent_id=it_shr000001&name=n.txt`, { method: 'PUT', headers: G, raw: 'x' });
  await err('unauthorized', `${GV}/roots`);
  await err('unauthorized', `${V}/roots`, { headers: { authorization: 'Files-Guest garbage' } });
  // create an upload_only and an edit share
  const mkShare = (role) => call('POST', `/api/v1/computer-files/cmp_cloud_4gb/shares`, { body: { root_id: 'rt_shared', item_id: 'it_shr000001', role, guest_wake: false }, status: 201, type: 'ShareResponse' });
  const up1 = (await mkShare('upload_only')).json.share; const ed1 = (await mkShare('edit')).json.share; const cm1 = (await mkShare('comment')).json.share;
  const tokOf = async (s) => { const r = await redeem(s.link_url.split('/').pop()); return { authorization: `Files-Guest ${r.json.capability_token}` }; };
  const GU = await tokOf(up1); const GE = await tokOf(ed1); const GC = await tokOf(cm1);
  await err('forbidden', `${GV}/roots/rt_shared/list?parent_id=it_shr000001`, { headers: GU }).then((x) => check(/only allows uploads/.test(x.json.message), 'upload_only cannot list'));
  await call('PUT', `${GV}/roots/rt_shared/content?parent_id=it_shr000001&name=drop.pdf`, { headers: GU, raw: 'guest file', type: 'MutationResponse' });
  await call('PUT', `${GV}/roots/rt_shared/content?parent_id=it_shr000001&name=queued.pdf`, { headers: { ...GU, ...scn('asleep') }, raw: 'while asleep', status: 202, type: 'QueuedDrop' });
  await err('guest_wake_not_allowed', `${GV}/items/${gl.items[0].id}/content`, { headers: { ...G, ...scn('asleep') } });
  await call('GET', `${GV}/roots/rt_shared/list?parent_id=it_shr000001`, { headers: { ...G, ...scn('asleep') }, type: 'ListResponse' });
  await call('PUT', `${GV}/roots/rt_shared/content?parent_id=it_shr000001&name=edit.txt`, { headers: GE, raw: 'edited', type: 'MutationResponse' });
  await call('POST', `${GV}/comments`, { headers: GC, body: { item_id: gl.items[0].id, body: 'ok' }, status: 201, type: 'Comment' });
  await err('forbidden', `${GV}/comments`, { method: 'POST', headers: G, body: { item_id: gl.items[0].id, body: 'no' } });
  const acc = (await call('GET', `/api/v1/computer-files/shares/${up1.share_id}/access`)).json;
  check(acc.some((a) => a.action === 'list' && a.result === 'denied'), 'audit records the denied list');
  await call('DELETE', `/api/v1/computer-files/shares/${ed1.share_id}`, { status: 204 });
  await err('share_revoked', `${GV}/roots`, { headers: GE });
  await call('GET', `/api/v1/computer-files/cmp_cloud_4gb/shares`, { type: 'SharesResponse' });
  await call('GET', `${V}/whoami`, { headers: { ...G, ...scn('guest') }, type: 'WhoAmIResponse' }).then((x) => check(x.json.principal.kind === 'guest', 'guest whoami'));
  await err('unauthorized', `${V}/roots`, { headers: scn('guest') });

  // ---------------------------------------------------------------- error scenario: every code reachable
  check(Object.keys(ERROR_STATUS).length === defs.get('ErrorCode').schema.enum.length && defs.get('ErrorCode').schema.enum.every((c) => ERROR_STATUS[c]), 'every ErrorCode has a status');
  for (const code of defs.get('ErrorCode').schema.enum) {
    const x = await call('GET', `${V}/roots`, { headers: scn('error', { 'x-files-stub-error': code }), status: ERROR_STATUS[code], type: 'ErrorBody' });
    check(x.json.code === code && x.json.error === code, `error scenario reaches ${code}`);
  }
  const wcr = await call('GET', `${V}/roots`, { headers: scn('error', { 'x-files-stub-error': 'wake_consent_required' }), status: 409, type: 'ErrorBody' });
  check(!!wcr.json.wake, 'forced wake_consent_required carries WakeHint');

  // reset restores the seed
  await call('POST', `/__stub/reset`);
  const afterReset = (await call('GET', `${V}/roots/rt_home/list?parent_id=${home.root_item_id}`, { type: 'ListResponse' })).json;
  check(afterReset.items.length === list.items.length, 'reset restores seeded state');

  await stub.close();
  console.log(bad ? `files-stub self-test: ${bad} FAILED of ${n} checks` : `files-stub self-test: ${n} checks passed`);
  return bad ? 1 : 0;
}

function wsConnect(port, path) {
  return new Promise((resolve, reject) => {
    const req = http.request({ host: '127.0.0.1', port, path, headers: { Connection: 'Upgrade', Upgrade: 'websocket', 'Sec-WebSocket-Version': '13', 'Sec-WebSocket-Key': crypto.randomBytes(16).toString('base64') } });
    req.on('upgrade', (_res, socket) => {
      const frames = []; let buf = Buffer.alloc(0);
      socket.on('data', (d) => {
        buf = Buffer.concat([buf, d]);
        while (buf.length >= 2) {
          let len = buf[1] & 127, off = 2;
          if (len === 126) { len = buf.readUInt16BE(2); off = 4; } else if (len === 127) { len = Number(buf.readBigUInt64BE(2)); off = 10; }
          if (buf.length < off + len) return;
          if ((buf[0] & 15) === 1) frames.push(JSON.parse(buf.subarray(off, off + len).toString()));
          buf = buf.subarray(off + len);
        }
      });
      resolve({
        frames: () => frames,
        send: (o) => { const p = Buffer.from(JSON.stringify(o)); const mask = crypto.randomBytes(4); const m = Buffer.from(p.map((b, i) => b ^ mask[i % 4])); const h = p.length < 126 ? Buffer.from([0x81, 0x80 | p.length]) : Buffer.from([0x81, 0x80 | 126, p.length >> 8, p.length & 255]); socket.write(Buffer.concat([h, mask, m])); },
        close: () => socket.destroy(),
      });
    });
    req.on('error', reject); req.end();
  });
}
