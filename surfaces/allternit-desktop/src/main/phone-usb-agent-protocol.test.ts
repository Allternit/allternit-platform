import { describe, expect, it } from 'vitest';
import {
  ReferenceBoard,
  boardProof,
  commandLine,
  commandTag,
  encodeCommand,
  equalHex,
  isValidCombo,
  newKeyHex,
  newNonceHex,
  parseHello,
  parseReply,
  parseState,
  serverProof,
  sessionKey,
  sealType,
  stateTag,
  textKey,
  replyTag,
  FACE_NAMES,
  FULL_REFRESH_EVERY,
} from './phone-usb-agent-protocol.js';
import { renderFace } from './phone-usb-agent-face.js';

const ID = 'usba-a1b2c3d4e5f6';
const v1Board = (key: string) => new ReferenceBoard(ID, key, '1.0.0', 1);

/** Desktop's half of the handshake, by hand. */
function handshake(board: ReferenceBoard, key: string) {
  const hello = parseHello(board.hello())!;
  const sn = newNonceHex();
  const reply = board.receive(`challenge ${sn} ${serverProof(key, ID, hello.boardNonce, sn)}`);
  return { hello, sn, reply, sk: sessionKey(key, hello.boardNonce, sn) };
}

describe('USBA/1 protocol', () => {
  it('parses a well-formed hello and rejects anything else', () => {
    const board = v1Board(newKeyHex());
    expect(parseHello(board.hello())).toMatchObject({ version: 1, boardId: ID, firmware: '1.0.0', resetReason: 'power-on' });
    expect(parseHello('hello usba/3 usba-a1b2c3d4e5f6 1.0.0 power-on ' + 'a'.repeat(32))).toBeNull();
    expect(parseHello('hello usba/1 board-1 1.0.0 power-on ' + 'a'.repeat(32))).toBeNull();
    expect(parseHello('hello usba/1 usba-a1b2c3d4e5f6 1.0.0 power-on short')).toBeNull();
    expect(parseHello('hello usba/1 usba-a1b2c3d4e5f6 1.0.0 power-on ' + 'a'.repeat(32) + ' extra')).toBeNull();
  });

  it('mutual handshake: the board proves the key back and both derive the same session key', () => {
    const key = newKeyHex();
    const board = v1Board(key);
    const { hello, sn, reply } = handshake(board, key);
    expect(reply).toEqual([`auth ${boardProof(key, ID, hello.boardNonce, sn)}`]);
  });

  it('a board drops a server that does not hold its key', () => {
    const board = v1Board(newKeyHex());
    const hello = parseHello(board.hello())!;
    const sn = newNonceHex();
    expect(board.receive(`challenge ${sn} ${serverProof(newKeyHex(), ID, hello.boardNonce, sn)}`)).toBe('drop');
  });

  it('commands need a valid tag, strictly increasing sequence, and an armed board', () => {
    const key = newKeyHex();
    const board = v1Board(key);
    const { sk } = handshake(board, key);
    expect(board.receive('ready')).toEqual(['s armed=0 usb=mounted']);

    expect(board.receive(commandLine(sk, 1, 'click'))).toEqual(['r 1 err disarmed']);
    board.armed = true;
    expect(board.receive(commandLine(sk, 2, 'click'))).toEqual(['r 2 ok']);
    // Replay of an old sequence number is refused, never executed.
    expect(board.receive(commandLine(sk, 2, 'click'))).toEqual(['r 2 err replay']);
    // A tag made with another session's key drops the connection.
    expect(board.receive(commandLine(newKeyHex(), 3, 'click'))).toBe('drop');
    // A tag for one command can't be moved onto another.
    expect(board.receive(`c 4 home ${commandTag(sk, 4, 'click')}`)).toBe('drop');
    expect(board.log).toEqual(['click']);
  });

  it('a recorded session cannot be replayed into a new one', () => {
    const key = newKeyHex();
    const board = v1Board(key);
    const first = handshake(board, key);
    board.armed = true;
    const recorded = commandLine(first.sk, 1, 'click');
    board.receive(recorded);
    handshake(board, key); // reconnect: fresh nonces
    expect(board.receive(recorded)).toBe('drop');
  });

  it('disarm is always accepted and stops input', () => {
    const key = newKeyHex();
    const board = v1Board(key);
    const { sk } = handshake(board, key);
    board.armed = true;
    expect(board.receive(commandLine(sk, 1, 'disarm'))).toEqual(['r 1 ok', 's armed=0 usb=mounted']);
    expect(board.receive(commandLine(sk, 2, 'click'))).toEqual(['r 2 err disarmed']);
  });

  it('bounds every command', () => {
    expect(encodeCommand({ op: 'move', dx: 4000, dy: -4000 })).toBe('move 4000 -4000');
    expect(encodeCommand({ op: 'move', dx: 4001, dy: 0 })).toBeNull();
    expect(encodeCommand({ op: 'move', dx: 1.5, dy: 0 })).toBeNull();
    expect(encodeCommand({ op: 'type', text: 'Hi there' })).toBe('type 4869207468657265');
    expect(encodeCommand({ op: 'type', text: 'héllo' })).toBeNull();
    expect(encodeCommand({ op: 'type', text: 'x'.repeat(201) })).toBeNull();
    expect(encodeCommand({ op: 'key', combo: 'cmd+space' })).toBe('key cmd+space');
    expect(encodeCommand({ op: 'key', combo: 'cmd+alt+ctrl+shift+delete' })).toBeNull();
    expect(encodeCommand({ op: 'face', name: 'needs_you' })).toBe('face needs_you');
    expect(encodeCommand({ op: 'face', name: 'angry' as never })).toBeNull();
    expect(encodeCommand({ op: 'prog', n: 0 })).toBe('prog 0');
    expect(encodeCommand({ op: 'prog', n: 5 })).toBe('prog 5');
    expect(encodeCommand({ op: 'prog', n: 6 })).toBeNull();
    expect(encodeCommand({ op: 'prog', n: -1 })).toBeNull();
    expect(encodeCommand({ op: 'prog', n: 1.5 })).toBeNull();
    expect(encodeCommand({ op: 'label', text: 'Hi' })).toBe('label 4869');
    expect(encodeCommand({ op: 'label', text: '' })).toBe('label -');
    expect(encodeCommand({ op: 'label', text: 'x'.repeat(24) })).toBe(`label ${'78'.repeat(24)}`);
    expect(encodeCommand({ op: 'label', text: 'x'.repeat(25) })).toBeNull();
    expect(encodeCommand({ op: 'label', text: 'héllo' })).toBeNull();
    expect(isValidCombo('enter')).toBe(true);
    expect(isValidCombo('cmd;rm')).toBe(false);
  });

  it('parses replies and state lines strictly', () => {
    expect(parseReply('r 7 ok')).toEqual({ seq: 7, ok: true });
    expect(parseReply('r 7 err disarmed')).toEqual({ seq: 7, ok: false, reason: 'disarmed' });
    expect(parseReply('r x ok')).toBeNull();
    expect(parseState('s armed=1 usb=asleep')).toEqual({ armed: true, usb: 'asleep' });
    expect(parseState('s armed=2 usb=asleep')).toBeNull();
    expect(equalHex('ab', 'ab')).toBe(true);
    expect(equalHex('ab', 'zz')).toBe(false);
  });
});

/** Desktop's half of a USBA/2 handshake, by hand. */
function handshake2(board: ReferenceBoard, key: string) {
  const hello = parseHello(board.hello())!;
  const sn = newNonceHex();
  const reply = board.receive(`challenge ${sn} ${serverProof(key, ID, hello.boardNonce, sn, 2)}`);
  return { hello, sn, reply, sk: sessionKey(key, hello.boardNonce, sn, 2), ek: textKey(key, hello.boardNonce, sn) };
}

/** Every command line a Desktop (or a relay holding SK) could send, valid or not. */
const ALL_COMMANDS = [
  'ping', 'disarm', 'home', 'move 10 10', 'press', 'release', 'click', 'scroll 1 0', 'type 6869', 'key enter',
  'face default', 'face wink', 'face working', 'face needs_you', 'face stopped', 'face angry',
  'prog 0', 'prog 3', 'prog 5', 'prog 6', 'prog x', 'label 4869', 'label -', 'label zz',
  'typex 00', 'arm', 'armed', 'button', 'set armed=1', 'prov set key 00', 'unknown',
];

describe('USBA/2 protocol', () => {
  it('binds the version into the handshake: a relay that rewrites usba/2 to usba/1 breaks it', () => {
    const key = newKeyHex();
    const board = new ReferenceBoard(ID, key);
    const hello = parseHello(board.hello())!;
    expect(hello).toMatchObject({ version: 2, firmware: '1.1.0' });
    const sn = newNonceHex();
    // Desktop fooled into USBA/1 labels: the board refuses its proof.
    expect(board.receive(`challenge ${sn} ${serverProof(key, ID, hello.boardNonce, sn, 1)}`)).toBe('drop');
    // A board fooled the other way answers with a v2 proof Desktop's v1 check does not accept.
    const ok = handshake2(board, key);
    expect(ok.reply).toEqual([`auth ${boardProof(key, ID, ok.hello.boardNonce, ok.sn, 2)}`]);
    expect(boardProof(key, ID, ok.hello.boardNonce, ok.sn, 2)).not.toBe(boardProof(key, ID, ok.hello.boardNonce, ok.sn, 1));
  });

  it('after a fresh hello, no sequence of commands leaves the board armed', () => {
    const key = newKeyHex();
    const board = new ReferenceBoard(ID, key);
    const { sk } = handshake2(board, key);
    board.receive('ready');
    let seq = 0;
    // Every command in order, then 300 more in a fixed pseudo-random order.
    let x = 12345;
    const order = [...ALL_COMMANDS.keys(), ...Array.from({ length: 300 }, () => ((x = (x * 1103515245 + 12345) % 2 ** 31) % ALL_COMMANDS.length))];
    for (const i of order) {
      seq += 1;
      const out = board.receive(commandLine(sk, seq, ALL_COMMANDS[i]));
      expect(out).not.toBe('drop');
      expect(board.armed, `after "${ALL_COMMANDS[i]}"`).toBe(false);
      for (const line of out as string[]) expect(line).not.toMatch(/armed=1/);
    }
    expect(board.log).toEqual([]);
  });

  it('face, prog and label answer ok while disarmed, record nothing in log, and only the button arms', () => {
    const key = newKeyHex();
    const board = new ReferenceBoard(ID, key);
    const { sk } = handshake2(board, key);
    const [ready] = board.receive('ready') as string[];
    expect(parseState(ready, 2, sk)).toMatchObject({ armed: false, face: 'default', prog: 0, dock: false, hid: 'usb', link: 'lan', n: 1 });

    const r1 = board.receive(commandLine(sk, 1, 'face working')) as string[];
    expect(parseReply(r1[0], 2, sk)).toEqual({ seq: 1, ok: true });
    expect(parseState(r1[1], 2, sk)).toMatchObject({ armed: false, face: 'working', n: 2 });
    const r2 = board.receive(commandLine(sk, 2, 'prog 3')) as string[];
    expect(parseReply(r2[0], 2, sk)).toEqual({ seq: 2, ok: true });
    expect(parseState(r2[1], 2, sk)).toMatchObject({ prog: 3, n: 3 });
    // A label changes no state field, so no state line follows.
    const r3 = board.receive(commandLine(sk, 3, 'label 4f6e20697420697421')) as string[];
    expect(r3).toHaveLength(1);
    expect(parseReply(r3[0], 2, sk)).toEqual({ seq: 3, ok: true });
    expect(board.label).toBe('On it it!');
    expect(parseReply((board.receive(commandLine(sk, 4, 'face angry')) as string[])[0], 2, sk)).toEqual({ seq: 4, ok: false, reason: 'unknown face' });
    expect(parseReply((board.receive(commandLine(sk, 5, 'prog 6')) as string[])[0], 2, sk)).toEqual({ seq: 5, ok: false, reason: 'out of bounds' });
    expect(parseReply((board.receive(commandLine(sk, 6, 'prog x')) as string[])[0], 2, sk)).toEqual({ seq: 6, ok: false, reason: 'bad numbers' });
    expect(parseReply((board.receive(commandLine(sk, 7, 'label 7f')) as string[])[0], 2, sk)).toEqual({ seq: 7, ok: false, reason: 'bad text' });
    expect(board.armed).toBe(false);
    expect(board.log).toEqual([]);
    // The panel shows what the renderer makes for that state.
    expect(Buffer.from(board.display.last()!).equals(Buffer.from(renderFace('working', 3, false)))).toBe(true);

    // The button arms (face goes back to default); the stop input only disarms.
    const armedLine = board.pressButton()!;
    expect(parseState(armedLine, 2, sk)).toMatchObject({ armed: true, face: 'default', prog: 3 });
    expect(Buffer.from(board.display.last()!).equals(Buffer.from(renderFace('default', 3, true)))).toBe(true);
    const stopped = board.pressStop()!;
    expect(parseState(stopped, 2, sk)).toMatchObject({ armed: false, face: 'stopped', prog: 0 });
    expect(board.display.frames[board.display.frames.length - 1].full).toBe(true);
    expect(parseState(board.pressStop()!, 2, sk)).toMatchObject({ armed: false });
  });

  it('a reply or state line with one changed byte fails to parse', () => {
    const key = newKeyHex();
    const board = new ReferenceBoard(ID, key);
    const { sk } = handshake2(board, key);
    const [state] = board.receive('ready') as string[];
    board.pressButton();
    const [reply] = board.receive(commandLine(sk, 1, 'click')) as string[];
    expect(parseReply(reply, 2, sk)).toEqual({ seq: 1, ok: true });
    expect(reply).toBe(`r 1 ok ${replyTag(sk, 1, 'ok')}`);
    // A relay turning an error into ok, or changing the sequence number.
    const forgedOk = `r 2 ok ${replyTag(sk, 2, 'err disarmed')}`;
    expect(parseReply(forgedOk, 2, sk)).toBeNull();
    expect(parseReply(reply.replace('r 1 ', 'r 9 '), 2, sk)).toBeNull();
    for (let i = 0; i < reply.length; i += 1) {
      const flipped = reply.slice(0, i) + String.fromCharCode(reply.charCodeAt(i) ^ 1) + reply.slice(i + 1);
      expect(parseReply(flipped, 2, sk), `reply byte ${i}`).toBeNull();
    }
    expect(parseState(state, 2, sk)).not.toBeNull();
    expect(parseState(state.replace('armed=0', 'armed=1'), 2, sk)).toBeNull();
    for (let i = 0; i < state.length; i += 1) {
      const flipped = state.slice(0, i) + String.fromCharCode(state.charCodeAt(i) ^ 1) + state.slice(i + 1);
      expect(parseState(flipped, 2, sk), `state byte ${i}`).toBeNull();
    }
    // Unsigned USBA/1 lines are not accepted on a v2 session.
    expect(parseReply('r 1 ok', 2, sk)).toBeNull();
    expect(parseState('s armed=1 usb=mounted', 2, sk)).toBeNull();
    // Unknown key=value tokens before n= are covered by the tag and ignored.
    const body = 'armed=1 usb=mounted dock=1 hid=usb link=cloud face=wink prog=2 bat=80 n=9';
    expect(parseState(`s ${body} ${stateTag(sk, body)}`, 2, sk)).toEqual({ armed: true, usb: 'mounted', dock: true, hid: 'usb', link: 'cloud', face: 'wink', prog: 2, n: 9 });
  });

  it('a recorded state line replayed into a new session fails', () => {
    const key = newKeyHex();
    const board = new ReferenceBoard(ID, key);
    handshake2(board, key);
    board.receive('ready');
    const recorded = board.pressButton()!; // "armed=1", signed under the first session key
    board.pressButton();
    const second = handshake2(board, key);
    board.receive('ready');
    expect(parseState(recorded, 2, second.sk)).toBeNull();
  });

  it('typex opens to the same text, and a wrong key or moved sequence answers err bad text', () => {
    const key = newKeyHex();
    const board = new ReferenceBoard(ID, key);
    const { sk, ek } = handshake2(board, key);
    board.receive('ready');
    board.pressButton();
    const sealed = sealType(ek, 1, 'hunter2 p@ss');
    expect(sealed).not.toContain(Buffer.from('hunter2').toString('hex'));
    expect(sealed.length - 'typex '.length).toBe(2 * (12 + 16));
    expect(parseReply((board.receive(commandLine(sk, 1, sealed)) as string[])[0], 2, sk)).toEqual({ seq: 1, ok: true });
    expect(board.log).toEqual([`type ${Buffer.from('hunter2 p@ss').toString('hex')}`]);
    // Sealed under another key, but tagged correctly: the tag passes, GCM does not.
    const wrong = sealType(newKeyHex(), 2, 'hunter2 p@ss');
    expect(parseReply((board.receive(commandLine(sk, 2, wrong)) as string[])[0], 2, sk)).toEqual({ seq: 2, ok: false, reason: 'bad text' });
    // Sealed for seq 3 but sent as seq 4: the IV and AAD differ.
    const moved = sealType(ek, 3, 'abc');
    expect(parseReply((board.receive(commandLine(sk, 4, moved)) as string[])[0], 2, sk)).toEqual({ seq: 4, ok: false, reason: 'bad text' });
    expect(board.log).toHaveLength(1);
    expect(() => sealType(ek, 5, 'héllo')).toThrow(RangeError);
    // A v2 board still takes plain `type`.
    expect(parseReply((board.receive(commandLine(sk, 6, 'type 6869')) as string[])[0], 2, sk)).toEqual({ seq: 6, ok: true });
  });

  it('a version 1 board still completes the handshake and runs a command', () => {
    const key = newKeyHex();
    const board = v1Board(key);
    const { reply, sk, hello, sn } = handshake(board, key);
    expect(hello.version).toBe(1);
    expect(reply).toEqual([`auth ${boardProof(key, ID, hello.boardNonce, sn)}`]);
    expect(board.receive('ready')).toEqual(['s armed=0 usb=mounted']);
    board.armed = true;
    expect(board.receive(commandLine(sk, 1, 'click'))).toEqual(['r 1 ok']);
    expect(board.log).toEqual(['click']);
  });

  it('shows a full refresh at boot, for stopped, and after every 20 partial refreshes', () => {
    const key = newKeyHex();
    const board = new ReferenceBoard(ID, key);
    expect(board.display.frames.map((f) => f.full)).toEqual([true]);
    const { sk } = handshake2(board, key);
    board.receive('ready');
    let seq = 0;
    for (let i = 0; i < FULL_REFRESH_EVERY + 1; i += 1) board.receive(commandLine(sk, ++seq, `prog ${(i % 5) + 1}`));
    const fulls = board.display.frames.map((f) => f.full);
    expect(fulls.slice(1, FULL_REFRESH_EVERY + 1).every((f) => !f)).toBe(true);
    expect(fulls[FULL_REFRESH_EVERY + 1]).toBe(true);
    // Sending the face already shown does not refresh the panel.
    const before = board.display.frames.length;
    board.receive(commandLine(sk, ++seq, 'face default'));
    expect(board.display.frames.length).toBe(before);
    expect(FACE_NAMES).toHaveLength(5);
  });
});
