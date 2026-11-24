#!/usr/bin/env node
// Files drive protocol v2 stub server. See README.md.
//   node tools/files-stub/server.mjs [--port 47820] [--scenario awake]
//   node tools/files-stub/server.mjs --self-test
import { createStub, SCENARIOS } from './stub.mjs';

const argv = process.argv.slice(2);
const opt = (n) => (argv.includes(n) ? argv[argv.indexOf(n) + 1] : undefined);

if (argv.includes('--self-test')) {
  const { runSelfTest } = await import('./selftest.mjs');
  process.exit(await runSelfTest());
}
if (argv.includes('--help')) {
  console.log(`files-stub: Files drive v2 stub\n  --port N        default env FILES_STUB_PORT or 47820\n  --scenario S    ${SCENARIOS.join(' | ')} (env FILES_STUB_SCENARIO)\n  --self-test     run the scripted contract check and exit`);
  process.exit(0);
}
const stub = createStub({ port: Number(opt('--port') || process.env.FILES_STUB_PORT || 47820), scenario: opt('--scenario') });
const port = await stub.listen();
console.log(`files-stub listening on http://127.0.0.1:${port}  scenario=${stub.cfg.scenario}  computer=${stub.cfg.computerId}`);
console.log(`runtime routes  http://127.0.0.1:${port}/api/v1/files/v2/*`);
console.log(`control         http://127.0.0.1:${port}/__stub/state`);
