// One linked-device session: Baileys socket lifecycle, QR, send, inbound policy + forward.
import fs from 'node:fs';
import path from 'node:path';
import { useEncryptedAuthState } from './auth-state.mjs';
import { readSealed, writeSealed } from './crypto.mjs';
import { bare, decide, textOf, isGroup, WARNING } from './policy.mjs';

const MAX_PENDING = 50;

export class Session {
  /**
   * @param {object} o
   * @param {() => Promise<object>} o.loadBaileys  returns the baileys module (injected so tests mock it)
   * @param {string} o.dataDir
   * @param {Buffer} o.key
   * @param {(event: object) => Promise<void>} o.forward  delivers a normalized inbound to allternit-api
   * @param {string[]} [o.owners]  extra approved numbers (digits) besides the account itself
   */
  constructor({ loadBaileys, dataDir, key, forward, owners = [], log = () => {} }) {
    this.loadBaileys = loadBaileys;
    this.authFile = path.join(dataDir, 'session.enc');
    this.policyFile = path.join(dataDir, 'policy.enc');
    this.key = key;
    this.forward = forward;
    this.log = log;
    this.status = 'idle'; // idle | connecting | qr | connected | logged_out
    this.qr = null;
    this.sock = null;
    this.sent = new Set(); // ids we sent, so self-chat echoes are not re-forwarded
    const saved = JSON.parse(readSealed(this.policyFile, key) ?? '{}');
    this.allow = new Set([...(saved.allow ?? []), ...owners.map(bare)]);
    this.pairing = new Map(Object.entries(saved.pairing ?? {})); // number -> { jid, firstText, at }
    this.starting = null;
  }

  info() {
    return { status: this.status, linked: Boolean(this.sock?.user), number: this.sock?.user ? bare(this.sock.user.id) : null, warning: WARNING };
  }

  persistPolicy() {
    writeSealed(this.policyFile, this.key, JSON.stringify({ allow: [...this.allow], pairing: Object.fromEntries(this.pairing) }));
  }

  ownIds() {
    const u = this.sock?.user;
    return u ? [u.id, u.lid].filter(Boolean) : [];
  }

  async start() {
    if (this.sock && (this.status === 'connected' || this.status === 'qr' || this.status === 'connecting')) return this.info();
    if (!this.starting) this.starting = this.#open().finally(() => (this.starting = null));
    await this.starting;
    return this.info();
  }

  async #open() {
    const b = await this.loadBaileys();
    const makeWASocket = b.makeWASocket ?? b.default;
    const { state, saveCreds } = useEncryptedAuthState(b, this.authFile, this.key);
    this.status = 'connecting';
    this.qr = null;
    const sock = makeWASocket({ auth: state, printQRInTerminal: false, markOnlineOnConnect: false, syncFullHistory: false });
    this.sock = sock;
    sock.ev.on('creds.update', saveCreds);
    sock.ev.on('connection.update', (u) => this.#onConnection(b, u));
    sock.ev.on('messages.upsert', (u) => this.#onMessages(u));
  }

  #onConnection(b, { connection, lastDisconnect, qr }) {
    if (qr) {
      this.qr = qr;
      this.status = 'qr';
    }
    if (connection === 'open') {
      this.qr = null;
      this.status = 'connected';
    }
    if (connection === 'close') {
      const code = lastDisconnect?.error?.output?.statusCode;
      this.sock = null;
      if (code === b.DisconnectReason?.loggedOut) {
        this.status = 'logged_out';
        this.qr = null;
        this.wipe();
      } else {
        this.status = 'connecting';
        this.log('wa-personal: connection closed, reconnecting', code);
        setTimeout(() => this.#open().catch((e) => this.log('wa-personal: reconnect failed', e.message)), 2000).unref?.();
      }
    }
  }

  async #onMessages({ messages = [], type }) {
    if (type !== 'notify') return;
    for (const msg of messages) {
      const id = msg.key?.id;
      if (!id || this.sent.has(id)) continue;
      const d = decide(msg, { own: this.ownIds(), allow: this.allow, pairing: this.pairing });
      if (d.action === 'pairing') {
        // Held for the owner to approve; never auto-reply to strangers (that is how numbers get flagged).
        const n = bare(d.jid);
        if (n && !this.pairing.has(n) && this.pairing.size < MAX_PENDING) {
          this.pairing.set(n, { jid: d.jid, firstText: textOf(msg).slice(0, 80), at: new Date().toISOString() });
          this.persistPolicy();
        }
        continue;
      }
      if (d.action !== 'forward') continue;
      const chat = msg.key.remoteJid;
      try {
        await this.forward({
          event: 'message',
          id,
          chat,
          isGroup: isGroup(chat),
          from: bare(msg.key.participant ?? (msg.key.fromMe ? this.ownIds()[0] : chat)),
          pushName: msg.pushName ?? null,
          text: textOf(msg),
          ts: Number(msg.messageTimestamp) || Math.floor(Date.now() / 1000),
        });
      } catch (e) {
        this.log('wa-personal: forward failed', e.message);
      }
    }
  }

  async send(to, text) {
    if (!this.sock || this.status !== 'connected') throw Object.assign(new Error('not_connected'), { code: 'not_connected' });
    const jid = to.includes('@') ? to : `${bare(to)}@s.whatsapp.net`;
    const res = await this.sock.sendMessage(jid, { text });
    const id = res?.key?.id;
    if (id) {
      this.sent.add(id);
      if (this.sent.size > 500) this.sent.delete(this.sent.values().next().value);
    }
    return { id: id ?? null };
  }

  approve(number) {
    const n = bare(number);
    if (!n) return false;
    this.allow.add(n);
    this.pairing.delete(n);
    this.persistPolicy();
    return true;
  }

  deny(number) {
    const had = this.pairing.delete(bare(number));
    if (had) this.persistPolicy();
    return had;
  }

  pending() {
    return [...this.pairing].map(([number, v]) => ({ number, ...v }));
  }

  wipe() {
    fs.rmSync(this.authFile, { force: true });
  }

  async logout() {
    try {
      await this.sock?.logout();
    } catch {}
    this.sock = null;
    this.status = 'logged_out';
    this.qr = null;
    this.wipe();
    return this.info();
  }
}
