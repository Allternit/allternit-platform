// node test/input.test.mjs
// Regression (Linux lab, 2026-11-19): with no python3 the input helper's spawn
// emitted an unhandled 'error' (ENOENT) that killed the whole server, and the
// desktop respawned it forever. The server must stay up with input off.
import assert from 'node:assert/strict';
import { once } from 'node:events';
import { InputBridge } from '../server/lib/input.mjs';

const lines = [];
const bridge = new InputBridge({ python: '/nonexistent/allternit-test/python3', log: (l) => lines.push(l) });
bridge.start();
const [reason] = await once(bridge, 'unavailable');
assert.equal(reason, '/nonexistent/allternit-test/python3 not found');
assert.equal(bridge.unavailable, reason);
assert.equal(bridge.child, null);
assert.ok(lines.some((l) => l.includes('phone input is off')), 'one log line says input is off');

const answer = await bridge.command({ type: 'ping' });
assert.equal(answer.ok, false, 'commands answer instead of hanging or throwing');
bridge.stop();
console.log('input.test: ok');
