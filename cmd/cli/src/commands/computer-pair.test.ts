import assert from 'node:assert/strict';
import test from 'node:test';
import { createServer } from 'node:net';
import { FACTORY_PEER_PORT, findMeshNode, meshHostname, meshNodeArgs, meshNodeLacksAlso, portOpen, serviceFiles, vncHelp } from './computer-pair.js';

test('finds mesh-node from the flag, env, PATH, or Allternit Desktop', () => {
  const has = (set: string[]) => (p: string) => set.includes(p);
  assert.equal(findMeshNode('/x/mesh-node', {}, has(['/x/mesh-node'])), '/x/mesh-node');
  assert.equal(findMeshNode(undefined, { ALLTERNIT_MESH_NODE_BIN: '/env/mesh-node' }, has(['/env/mesh-node'])), '/env/mesh-node');
  assert.equal(findMeshNode(undefined, { PATH: '/a:/b' }, has(['/b/mesh-node'])), '/b/mesh-node');
  assert.match(findMeshNode(undefined, {}, has(['/Applications/Allternit Desktop.app/Contents/Resources/bin/mesh-node'])) ?? '', /Allternit Desktop\.app/);
  assert.equal(findMeshNode(undefined, {}, has([])), null);
});

test('makes a mesh host name from the computer name', () => {
  assert.equal(meshHostname('Joe’s Studio Mac'), 'joe-s-studio-mac');
  assert.equal(meshHostname('!!!'), 'allternit-computer');
});

test('detects a listening VNC port', async () => {
  const server = createServer();
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const port = (server.address() as { port: number }).port;
  assert.equal(await portOpen(port), true);
  await new Promise<void>((resolve) => server.close(() => resolve()));
  assert.equal(await portOpen(port), false);
});

test('writes a login service for macOS and Linux', () => {
  const mac = serviceFiles('/usr/bin/node', '/cli.js', 'darwin');
  assert.match(mac?.path ?? '', /LaunchAgents\/com\.allternit\.computer\.plist$/);
  assert.match(mac?.content ?? '', /<string>serve<\/string>/);
  const linux = serviceFiles('/usr/bin/node', '/cli.js', 'linux');
  assert.match(linux?.content ?? '', /ExecStart=\/usr\/bin\/node \/cli\.js computers serve/);
  assert.equal(serviceFiles('node', 'cli', 'win32'), null);
});

test('explains how to turn on VNC', () => {
  assert.match(vncHelp('darwin'), /Screen Sharing/);
  assert.match(vncHelp('linux'), /5900/);
});

test('mesh-node listens on 5900 on the mesh and forwards to the local VNC port', () => {
  const base = { computerId: 'c', secret: 's', cloudUrl: 'u', name: 'Mail VPS', controlUrl: 'https://mesh', meshNode: '/bin/mesh-node' };
  const args = meshNodeArgs(base);
  assert.equal(args[args.indexOf('--forward') + 1], '5900');
  assert.equal(args[args.indexOf('--listen') + 1], '5900');
  const other = meshNodeArgs({ ...base, vncPort: 5942 });
  assert.equal(other[other.indexOf('--forward') + 1], '5942');
  assert.equal(other[other.indexOf('--listen') + 1], '5900');
});

test('mesh-node also forwards the Factory peer port, unless it is too old for --also', () => {
  const base = { computerId: 'c', secret: 's', cloudUrl: 'u', name: 'Mail VPS', controlUrl: 'https://mesh', meshNode: '/bin/mesh-node' };
  const args = meshNodeArgs(base);
  assert.equal(args[args.indexOf('--also') + 1], String(FACTORY_PEER_PORT));
  assert.equal(meshNodeArgs(base, { peer: false }).includes('--also'), false);
  assert.equal(meshNodeLacksAlso('flag provided but not defined: -also\nUsage of mesh-node:'), true);
  assert.equal(meshNodeLacksAlso('MESH_ERROR reason=bad key'), false);
});
