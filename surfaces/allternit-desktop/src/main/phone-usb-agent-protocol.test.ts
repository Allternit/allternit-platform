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
  stateLine,
  sessionKey,
} from './phone-usb-agent-protocol.js';

const ID = 'usba-a1b2c3d4e5f6';

/** Desktop's half of the handshake, by hand. */
function handshake(board: ReferenceBoard, key: string) {
  const hello = parseHello(board.hello())!;
  const sn = newNonceHex();
  const reply = board.receive(`challenge ${sn} ${serverProof(key, ID, hello.boardNonce, sn)}`);
  return { hello, sn, reply, sk: sessionKey(key, hello.boardNonce, sn) };
}

describe('USBA/1 protocol', () => {
  it('parses a well-formed hello and rejects anything else', () => {
    const board = new ReferenceBoard(ID, newKeyHex());
    expect(parseHello(board.hello())).toMatchObject({ boardId: ID, firmware: '1.1.0', resetReason: 'power-on' });
    expect(parseHello('hello usba/2 usba-a1b2c3d4e5f6 1.0.0 power-on ' + 'a'.repeat(32))).toBeNull();
    expect(parseHello('hello usba/1 board-1 1.0.0 power-on ' + 'a'.repeat(32))).toBeNull();
    expect(parseHello('hello usba/1 usba-a1b2c3d4e5f6 1.0.0 power-on short')).toBeNull();
    expect(parseHello('hello usba/1 usba-a1b2c3d4e5f6 1.0.0 power-on ' + 'a'.repeat(32) + ' extra')).toBeNull();
  });

  it('mutual handshake: the board proves the key back and both derive the same session key', () => {
    const key = newKeyHex();
    const board = new ReferenceBoard(ID, key);
    const { hello, sn, reply } = handshake(board, key);
    expect(reply).toEqual([`auth ${boardProof(key, ID, hello.boardNonce, sn)}`]);
  });

  it('a board drops a server that does not hold its key', () => {
    const board = new ReferenceBoard(ID, newKeyHex());
    const hello = parseHello(board.hello())!;
    const sn = newNonceHex();
    expect(board.receive(`challenge ${sn} ${serverProof(newKeyHex(), ID, hello.boardNonce, sn)}`)).toBe('drop');
  });

  it('commands need a valid tag, strictly increasing sequence, and an armed board', () => {
    const key = newKeyHex();
    const board = new ReferenceBoard(ID, key);
    const { sk } = handshake(board, key);
    expect(board.receive('ready')).toEqual(['s armed=0 usb=mounted mode=cable bt=off']);

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
    const board = new ReferenceBoard(ID, key);
    const first = handshake(board, key);
    board.armed = true;
    const recorded = commandLine(first.sk, 1, 'click');
    board.receive(recorded);
    handshake(board, key); // reconnect: fresh nonces
    expect(board.receive(recorded)).toBe('drop');
  });

  it('disarm is always accepted and stops input', () => {
    const key = newKeyHex();
    const board = new ReferenceBoard(ID, key);
    const { sk } = handshake(board, key);
    board.armed = true;
    expect(board.receive(commandLine(sk, 1, 'disarm'))).toEqual(['r 1 ok', 's armed=0 usb=mounted mode=cable bt=off']);
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
    expect(isValidCombo('enter')).toBe(true);
    expect(isValidCombo('cmd;rm')).toBe(false);
  });

  it('parses replies and state lines strictly', () => {
    expect(parseReply('r 7 ok')).toEqual({ seq: 7, ok: true });
    expect(parseReply('r 7 err disarmed')).toEqual({ seq: 7, ok: false, reason: 'disarmed' });
    expect(parseReply('r x ok')).toBeNull();
    // Firmware before 1.1.0 has no Bluetooth mode: its line parses as cable mode.
    expect(parseState('s armed=1 usb=asleep')).toEqual({ armed: true, usb: 'asleep', mode: 'cable', bt: 'off' });
    expect(parseState('s armed=1 usb=none mode=bt bt=connected')).toEqual({ armed: true, usb: 'none', mode: 'bt', bt: 'connected' });
    expect(parseState('s armed=1 usb=none mode=bt bt=lost')).toBeNull();
    expect(parseState('s armed=2 usb=asleep')).toBeNull();
    expect(equalHex('ab', 'ab')).toBe(true);
    expect(equalHex('ab', 'zz')).toBe(false);
  });
});

describe('USBA/1 Bluetooth mode', () => {
  const connect = () => {
    const key = newKeyHex();
    const board = new ReferenceBoard(ID, key);
    const { sk } = handshake(board, key);
    return { board, sk, n: 0, send(this: { board: ReferenceBoard; sk: string; n: number }, cmd: string) { return this.board.receive(commandLine(this.sk, ++this.n, cmd)); } };
  };

  it('encodes the mode, pair and forget commands, and nothing else', () => {
    expect(encodeCommand({ op: 'mode', mode: 'bt' })).toBe('mode bt');
    expect(encodeCommand({ op: 'mode', mode: 'cable' })).toBe('mode cable');
    expect(encodeCommand({ op: 'mode', mode: 'usb' as never })).toBeNull();
    expect(encodeCommand({ op: 'btpair' })).toBe('bt pair');
    expect(encodeCommand({ op: 'btforget' })).toBe('bt forget');
  });

  it('switching to Bluetooth disarms, starts pairing, and sends nothing until an iPhone is connected', () => {
    const c = connect();
    c.board.armed = true;
    expect(c.send('mode bt')).toEqual(['r 1 ok', 's armed=0 usb=mounted mode=bt bt=pairing']);
    c.board.armed = true; // the button
    expect(c.send('click')).toEqual(['r 2 err bt not connected']);
    c.board.setBtConnected(true);
    expect(c.send('click')).toEqual(['r 3 ok']);
    expect(c.board.log).toEqual(['click']);
    // The cable's state is irrelevant in Bluetooth mode.
    c.board.usb = 'none';
    expect(c.send('type 6869')).toEqual(['r 4 ok']);
  });

  it('disarmed means no reports in Bluetooth mode too, and a dropped link stops them', () => {
    const c = connect();
    c.send('mode bt');
    c.board.setBtConnected(true);
    expect(c.send('click')).toEqual(['r 2 err disarmed']);
    c.board.armed = true;
    c.board.setBtConnected(false);
    expect(c.board.bt).toBe('bonded');
    expect(c.send('move 5 5')).toEqual(['r 3 err bt not connected']);
    expect(c.board.log).toEqual([]);
  });

  it('pair and forget need Bluetooth mode; forget disarms and unbonds', () => {
    const c = connect();
    expect(c.send('bt pair')).toEqual(['r 1 err not in bluetooth mode']);
    c.send('mode bt');
    c.board.setBtConnected(true);
    c.board.armed = true;
    expect(c.send('bt forget')).toEqual(['r 3 ok', 's armed=0 usb=mounted mode=bt bt=pairing']);
    expect(c.board.bonded).toBe(false);
    expect(stateLine({ armed: false, usb: 'mounted', mode: 'cable', bt: 'off' })).toBe('s armed=0 usb=mounted mode=cable bt=off');
  });

  it('going back to cable mode turns Bluetooth off but keeps the bond', () => {
    const c = connect();
    c.send('mode bt');
    c.board.setBtConnected(true);
    expect(c.send('mode cable')).toEqual(['r 3 ok', 's armed=0 usb=mounted mode=cable bt=off']);
    expect(c.send('mode bt')).toEqual(['r 4 ok', 's armed=0 usb=mounted mode=bt bt=bonded']);
  });
});
