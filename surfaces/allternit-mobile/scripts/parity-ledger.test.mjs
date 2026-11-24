import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { run } from './parity-ledger.mjs';

function fixture() {
  const d = mkdtempSync(join(tmpdir(), 'parity-'));
  const root = join(d, 'mobile'), web = join(d, 'web');
  mkdirSync(join(root, 'ios/Features/Chat'), { recursive: true });
  writeFileSync(join(root, 'ios/Features/Chat/ChatView.swift'), '');
  mkdirSync(join(web, 'src/fabric-session/phone/screens'), { recursive: true });
  writeFileSync(join(web, 'src/fabric-session/phone/screens/ChatsScreen.tsx'), '');
  writeFileSync(join(web, 'src/fabric-session/phone/phone-nav.ts'),
    "export type PhoneSection = 'chats' | 'bots';\nexport type PhoneMode = 'home';\n");
  return { root, web, args: ['--root', root, '--web', web] };
}
const quiet = () => {};

test('first run writes todo rows, merge keeps hand edits and adds new screens', () => {
  const f = fixture();
  assert.equal(run(f.args, quiet), 0);
  const p = join(f.root, 'PARITY.md');
  let md = readFileSync(p, 'utf8');
  assert.match(md, /\| ChatView \| ios\/Features\/Chat\/ChatView.swift \| — \| — \| todo \|/);
  md = md.replace(/(\| ChatView \|.*?\| — \| )— \| todo \| — \|/, '$1ChatScreen.kt | proved | pr/1 |');
  writeFileSync(p, md);
  writeFileSync(join(f.root, 'ios/Features/Chat/NewSheet.swift'), '');
  assert.equal(run(f.args, quiet), 0);
  md = readFileSync(p, 'utf8');
  assert.match(md, /ChatScreen\.kt \| proved \| pr\/1/);
  assert.match(md, /\| NewSheet \| .*\| todo \| — \|/);
});

test('vanished source is kept as todo (source removed)', () => {
  const f = fixture();
  run(f.args, quiet);
  writeFileSync(join(f.root, 'ios/Features/Chat/NewSheet.swift'), '');
  run(f.args, quiet);
  rmSync(join(f.root, 'ios/Features/Chat/NewSheet.swift'));
  run(f.args, quiet);
  assert.match(readFileSync(join(f.root, 'PARITY.md'), 'utf8'), /\| NewSheet \|.*todo \(source removed\)/);
});

test('--check fails on a missing row and passes otherwise', () => {
  const f = fixture();
  run(f.args, quiet);
  assert.equal(run([...f.args, '--check'], quiet), 0);
  writeFileSync(join(f.root, 'ios/Features/Chat/MissingPage.swift'), '');
  const lines = [];
  assert.equal(run([...f.args, '--check'], (l) => lines.push(l)), 1);
  assert.ok(lines.join('\n').includes('MissingPage.swift'));
});
