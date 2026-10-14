// Shared macOS Messages transport for text/iMessage assistants (route 3 of the bot-project adapter brief).
// Sends through Messages.app via osascript (text passed as argv, never interpolated into the script) and reads the
// reply by polling ~/Library/Messages/chat.db read-only. macOS only; every other platform is inert.
import { execFile } from 'node:child_process';
import { existsSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { AsyncLocalStorage } from 'node:async_hooks';
import { randomUUID } from 'node:crypto';
import Database from 'better-sqlite3';
import { BaseAaiProvider, fail, ok, type AaiResult, type AgentSummary, type OpenContextInput, type MessageInput, type EventsInput, type EventsResult, type CursoredEvent } from '@allternit/agent-gateway';
import { agentCapabilityManifestSchema } from '@allternit/subscription-fabric-contracts';
import { isObject, type Json } from './paired-local.js';

export interface MessagesAdapterSpec {
  /** Adapter folder id, e.g. "iris". Also the vendor id and the agentId prefix. */
  adapterId: string;
  displayName: string;
  /** Only set when the vendor officially publishes the number/handle to text. */
  defaultHandle?: string;
}

export interface MessagesProviderOptions {
  env?: NodeJS.ProcessEnv;
  platform?: string;
  chatDbPath?: string;
  replyTimeoutMs?: number;
}

type Scope = { binding?: { id?: string; accountBindingId?: string }; credential?: { apiKey: string } };
type Reply = { reply: string; replyRowId: number };
type Ctx = {
  id: string; owner: string; handle: string; key: string; closed: boolean; busy: boolean;
  sends: Map<string, Promise<AaiResult<{ messageId: string; correlationId: string; reply: string; guarantee: 'best_effort' }>>>;
  sendTexts: Map<string, string>; events: CursoredEvent[];
};

const POLL_MS = 1_500;
const GATHER_MS = 3_000;
const DEFAULT_TIMEOUT_MS = 120_000;
const CHAT_DB = join(homedir(), 'Library', 'Messages', 'chat.db');
/** Handle keys with a send in flight. Module-level so every adapter that texts the same handle shares the lock. */
const busyHandles = new Set<string>();

const scopeStore = () => (globalThis as unknown as Record<symbol, AsyncLocalStorage<Scope> | undefined>)[Symbol.for('allternit.aai.callScope')]?.getStore();
const idLike = (v: unknown): v is string => typeof v === 'string' && /^[\w-]{1,200}$/.test(v);
const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

/** Emails are lowercased; phone numbers keep their leading + and digits (spaces, dashes, parens removed). */
export function normalizeHandle(raw: unknown): string | undefined {
  if (typeof raw !== 'string' || !raw.trim()) return undefined;
  const value = raw.trim();
  if (value.includes('@')) return /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(value) ? value.toLowerCase() : undefined;
  const compact = value.replace(/[\s().-]/g, '');
  return /^\+?\d{7,15}$/.test(compact) ? compact : undefined;
}
const isPhone = (handle: string) => /^\+?\d+$/.test(handle);
/** Chat.db stores phones as "+1..."; matching on the last 10 digits tolerates formatting differences. */
const handleKey = (handle: string) => (handle.includes('@') ? handle.toLowerCase() : handle.replace(/\D/g, '').slice(-10));

/** Best-effort decoder for the typedstream `attributedBody` blob: takes the NSString payload after the '+' type code. */
export function extractAttributedText(blob: Buffer | null | undefined): string {
  if (!blob || blob.length === 0) return '';
  let start = -1;
  for (const marker of [Buffer.from('NSMutableString'), Buffer.from('NSString')]) {
    const at = blob.indexOf(marker);
    if (at >= 0) { start = at + marker.length; break; }
  }
  if (start < 0) return '';
  try {
    let plus = -1;
    for (let i = start; i < Math.min(blob.length, start + 64); i++) if (blob[i] === 0x2b) { plus = i; break; }
    if (plus < 0) return '';
    let pos = plus + 1;
    let length: number;
    const tag = blob[pos];
    if (tag === 0x81) { length = blob.readUInt16LE(pos + 1); pos += 3; }
    else if (tag === 0x82) { length = blob.readUInt32LE(pos + 1); pos += 5; }
    else { length = tag; pos += 1; }
    return blob.subarray(pos, Math.min(blob.length, pos + length)).toString('utf8').replace(/￼/g, '').trim();
  } catch {
    return '';
  }
}

/** Opens chat.db read-only. Failure almost always means the gateway process lacks Full Disk Access. */
export function openChatDb(path: string = CHAT_DB): AaiResult<Database.Database> {
  if (!existsSync(path)) return fail('AUTH_REQUIRED', 'Messages history was not found on this computer. Sign in to Messages on the gateway Mac first.');
  try {
    return ok(new Database(path, { readonly: true, fileMustExist: true }));
  } catch {
    return fail('AUTH_REQUIRED', 'Allternit cannot read ~/Library/Messages/chat.db. Grant Full Disk Access to the gateway process in System Settings > Privacy & Security, then restart the gateway.', { retryable: false });
  }
}

const OSA_SCRIPT = `on run argv
  set theHandle to item 1 of argv
  set theText to item 2 of argv
  set theService to item 3 of argv
  tell application "Messages"
    if theService is "SMS" then
      set theSvc to first service whose service type is SMS
    else
      set theSvc to first service whose service type is iMessage
    end if
    set theBuddy to buddy theHandle of theSvc
    send theText to theBuddy
  end tell
end run`;

function osascriptSend(handle: string, text: string, service: 'iMessage' | 'SMS'): Promise<boolean> {
  return new Promise((resolve) => {
    // Arguments go to the run handler as argv; nothing is spliced into OSA_SCRIPT.
    execFile('osascript', ['-e', OSA_SCRIPT, handle, text, service], { timeout: 30_000, maxBuffer: 1024 * 1024 }, (error) => resolve(!error));
  });
}

type InboundRow = { rowid: number; text: string };
function inbound(db: Database.Database, handle: string, after: number): InboundRow[] {
  const key = handleKey(handle);
  const ids = (db.prepare('SELECT ROWID AS rowid, id FROM handle').all() as { rowid: number; id: string }[])
    .filter((row) => handleKey(row.id) === key)
    .map((row) => row.rowid);
  if (ids.length === 0) return [];
  const rows = db.prepare(
    `SELECT ROWID AS rowid, text, attributedBody FROM message
     WHERE handle_id IN (${ids.map(() => '?').join(',')}) AND is_from_me = 0 AND ROWID > ?
       AND (associated_message_type IS NULL OR associated_message_type = 0)
     ORDER BY ROWID ASC`,
  ).all(...ids, after) as { rowid: number; text: string | null; attributedBody: Buffer | null }[];
  return rows.map((row) => ({
    rowid: row.rowid,
    text: (row.text ?? '').replace(/￼/g, '').trim() || extractAttributedText(row.attributedBody),
  }));
}

/**
 * Sends one text and returns the assistant's reply. The baseline ROWID is read before sending, so only inbound
 * messages that arrive after this send are attributed to it. The handle is busy-locked for the whole exchange.
 */
export async function sendAndAwaitReply(opts: { handle: string; text: string; platform: string; chatDbPath?: string; timeoutMs?: number }): Promise<AaiResult<Reply>> {
  if (opts.platform !== 'darwin') return fail('LANE_BLOCKED', 'Messages adapters run only on macOS.');
  const key = handleKey(opts.handle);
  if (busyHandles.has(key)) return fail('CONTEXT_BUSY', 'This handle already has an active Allternit request in Messages.');
  const access = openChatDb(opts.chatDbPath);
  if (!access.ok) return access;
  const db = access.value;
  busyHandles.add(key);
  try {
    const baseline = (db.prepare('SELECT COALESCE(MAX(ROWID), 0) AS m FROM message').get() as { m: number }).m;
    let sent = await osascriptSend(opts.handle, opts.text, 'iMessage');
    if (!sent && isPhone(opts.handle)) sent = await osascriptSend(opts.handle, opts.text, 'SMS');
    if (!sent) return fail('VENDOR_UNAVAILABLE', 'Messages could not send. Check that Allternit may control Messages (Privacy & Security > Automation) and that the Messages account is signed in.', { retryable: false });

    const deadline = Date.now() + (opts.timeoutMs ?? DEFAULT_TIMEOUT_MS);
    let rows: InboundRow[] = [];
    while (Date.now() < deadline) {
      await sleep(POLL_MS);
      rows = inbound(db, opts.handle, baseline);
      if (rows.some((row) => row.text)) break;
    }
    if (!rows.some((row) => row.text)) return fail('VENDOR_UNAVAILABLE', 'No reply arrived in Messages before the timeout. Check the thread before retrying.', { retryable: false });
    await sleep(GATHER_MS);
    rows = inbound(db, opts.handle, baseline).filter((row) => row.text);
    return ok({ reply: rows.map((row) => row.text).join('\n'), replyRowId: rows[rows.length - 1].rowid });
  } catch {
    return fail('VENDOR_UNAVAILABLE', 'The Messages reply could not be read. Check Full Disk Access for the gateway process.', { retryable: false });
  } finally {
    busyHandles.delete(key);
    db.close();
  }
}

const capabilitiesFor = (adapterId: string) => agentCapabilityManifestSchema.parse({
  vendor: adapterId, adapterId, lane: 'local', guarantee: 'best_effort',
  context: { supported: true, resume: false, parallel: false, maxParallel: 1, isolation: 'shared' },
  messaging: { send: true, stream: false, steer: false, interrupt: false, cancel: false },
  memory: { read: false, write: false, snapshot: false, opaque: true }, tools: { tools: false, mcp: false, plugins: false, connectors: false },
  tasks: { list: false, schedule: false, cancel: false, background: false }, approvals: { read: false, respond: false, exact: false },
  computer: { view: false, control: false, takeover: false }, artifacts: { read: false, write: false, export: false },
  events: { native: true, polling: false, transcriptDerived: false, replay: true }, runtime: { alwaysOn: false, localRequired: false, cloud: false },
});

const scopeOwner = () => scopeStore()?.binding?.accountBindingId ?? scopeStore()?.binding?.id ?? 'local';

export class MessagesProvider extends BaseAaiProvider {
  readonly adapterId: string;
  private readonly env: NodeJS.ProcessEnv;
  private readonly platform: string;
  private readonly chatDbPath?: string;
  private readonly timeoutMs: number;
  private readonly contexts = new Map<string, Ctx>();
  private readonly agentId: string;

  constructor(private readonly spec: MessagesAdapterSpec, options: MessagesProviderOptions = {}) {
    super();
    this.adapterId = spec.adapterId;
    this.agentId = `${spec.adapterId}:assistant`;
    this.env = options.env ?? process.env;
    this.platform = options.platform ?? process.platform;
    this.chatDbPath = options.chatDbPath;
    this.timeoutMs = options.replyTimeoutMs ?? DEFAULT_TIMEOUT_MS;
  }

  private enabled(): AaiResult<true> {
    if (this.platform !== 'darwin') return fail('LANE_BLOCKED', 'The Messages adapter runs only on macOS.', { retryable: false });
    if (this.env.SUBS_GATEWAY_MESSAGES_ENABLED !== '1') return fail('AUTH_REQUIRED', 'Messages adapters are opt-in. Set SUBS_GATEWAY_MESSAGES_ENABLED=1 on the gateway computer.', { retryable: false });
    return ok(true);
  }

  /** Per-account handle (credential JSON {"handle":"..."}) beats the deployment env override, which beats the published default. */
  private handle(): AaiResult<string> {
    let fromAccount: unknown;
    const raw = scopeStore()?.credential?.apiKey;
    if (raw) { try { const value: unknown = JSON.parse(raw); if (isObject(value)) fromAccount = value.handle; } catch { /* plain credential, no handle */ } }
    const handle = normalizeHandle(fromAccount ?? this.env[`SUBS_GATEWAY_${this.spec.adapterId.toUpperCase()}_HANDLE`] ?? this.spec.defaultHandle);
    return handle ? ok(handle) : fail('AUTH_REQUIRED', `Enter the phone number (with country code) or iMessage address you text ${this.spec.displayName} from.`, { retryable: false });
  }

  private configured(): AaiResult<{ handle: string; owner: string }> {
    const on = this.enabled();
    if (!on.ok) return on;
    const handle = this.handle();
    if (!handle.ok) return handle;
    return ok({ handle: handle.value, owner: scopeOwner() });
  }

  private live(id: string): AaiResult<Ctx> {
    const ctx = [...this.contexts.values()].find((c) => c.id === id);
    const owner = scopeOwner();
    return ctx && !ctx.closed && ctx.owner === owner ? ok(ctx) : fail('CONTEXT_NOT_FOUND', 'This Messages context is not open for the current account.');
  }

  private summary(): AgentSummary {
    return { agentId: this.agentId, displayName: this.spec.displayName, vendor: this.spec.adapterId, state: 'ready' };
  }

  async list() {
    if (!this.configured().ok) return ok([] as AgentSummary[]);
    return ok([this.summary()]);
  }

  async get(agentId: string) {
    const cfg = this.configured();
    if (!cfg.ok) return cfg;
    if (agentId !== this.agentId) return fail('CONTEXT_NOT_FOUND', `No ${this.spec.displayName} agent with that id.`);
    return ok({ ...this.summary(), remoteIds: { handleKey: handleKey(cfg.value.handle) }, capabilities: capabilitiesFor(this.spec.adapterId) });
  }

  async capabilities(agentId: string) {
    const result = await this.get(agentId);
    return result.ok ? ok(result.value.capabilities) : result;
  }

  async identity(agentId: string) {
    const result = await this.get(agentId);
    return result.ok ? ok({ agentId, displayName: this.spec.displayName, vendor: this.spec.adapterId }) : result;
  }

  async contextOpen(input: OpenContextInput) {
    if (input.agentId !== this.agentId) return fail('CONTEXT_NOT_FOUND', `No ${this.spec.displayName} agent with that id.`);
    if (input.adoptContextId) return fail('UNSUPPORTED', 'Messages contexts cannot be adopted after gateway detachment.', { retryable: false });
    const cfg = this.configured();
    if (!cfg.ok) return cfg;
    const access = openChatDb(this.chatDbPath);
    if (!access.ok) return access;
    access.value.close();
    // One Allternit context per handle: reuse the open one so every reply is attributable to a single thread.
    const key = `${cfg.value.owner}:${handleKey(cfg.value.handle)}`;
    const existing = this.contexts.get(key);
    if (existing && !existing.closed) return ok({ contextId: existing.id, isolation: 'shared' as const, guarantee: 'best_effort' as const, resumed: false });
    const ctx: Ctx = { id: randomUUID(), owner: cfg.value.owner, handle: cfg.value.handle, key: handleKey(cfg.value.handle), closed: false, busy: false, sends: new Map(), sendTexts: new Map(), events: [] };
    this.contexts.set(key, ctx);
    return ok({ contextId: ctx.id, isolation: 'shared' as const, guarantee: 'best_effort' as const, resumed: false });
  }

  async contextMessage(input: MessageInput) {
    const ctx = this.live(input.contextId);
    if (!ctx.ok) return ctx;
    if (!input.text.trim() || !idLike(input.correlationId)) return fail('LANE_BLOCKED', 'A nonempty message and stable send ID are required.');
    const prior = ctx.value.sends.get(input.correlationId);
    if (prior) return ctx.value.sendTexts.get(input.correlationId) === input.text ? prior : fail('POLICY_DENIED', 'This send ID already belongs to a different message.');
    if (ctx.value.busy || busyHandles.has(ctx.value.key)) return fail('CONTEXT_BUSY', `${this.spec.displayName} already has an active request on this handle.`);
    ctx.value.busy = true;
    ctx.value.sendTexts.set(input.correlationId, input.text);
    const promise = this.send(ctx.value, input).finally(() => { ctx.value.busy = false; });
    ctx.value.sends.set(input.correlationId, promise);
    return promise;
  }

  private async send(ctx: Ctx, input: MessageInput) {
    const result = await sendAndAwaitReply({ handle: ctx.handle, text: input.text, platform: this.platform, chatDbPath: this.chatDbPath, timeoutMs: this.timeoutMs });
    if (!result.ok) return result;
    const messageId = `messages-${result.value.replyRowId}`;
    ctx.events.push({ cursor: String(ctx.events.length + 1), event: { type: 'agent.message.completed', botId: `${this.spec.adapterId}:assistant`, threadId: ctx.id, generationId: '1', source: 'vendor', vendor: this.spec.adapterId, adapter: this.spec.adapterId, lane: 'local', remoteContextId: ctx.key, remoteEventId: messageId, correlationId: input.correlationId, causationId: input.correlationId, guarantee: 'best_effort', at: new Date().toISOString(), payload: { reply: result.value.reply, messageId } } });
    return ok({ messageId, correlationId: input.correlationId, reply: result.value.reply, guarantee: 'best_effort' as const });
  }

  async events(input: EventsInput): Promise<AaiResult<EventsResult>> {
    const ctx = this.live(input.contextId);
    if (!ctx.ok) return ctx;
    const after = Number(input.cursor ?? 0);
    if (!Number.isSafeInteger(after) || after < 0 || after > ctx.value.events.length) return fail('CONTEXT_NOT_FOUND', 'Invalid Messages event cursor.');
    const rows = ctx.value.events.slice(after, after + Math.max(1, Math.min(input.limit ?? 100, 500)));
    return ok({ events: rows, nextCursor: String(after + rows.length) });
  }

  async contextClose(input: { contextId: string }) {
    const ctx = this.live(input.contextId);
    if (!ctx.ok) return ctx;
    if (ctx.value.busy) return fail('CONTEXT_BUSY', `Wait for the ${this.spec.displayName} reply before detaching.`);
    ctx.value.closed = true;
    return ok({ closed: true });
  }

  async health() {
    const cfg = this.configured();
    if (!cfg.ok) return ok({ status: 'down' as const, lane: 'local', detail: cfg.error.humanMessage });
    const access = openChatDb(this.chatDbPath);
    if (!access.ok) return ok({ status: 'down' as const, lane: 'local', detail: access.error.humanMessage });
    access.value.close();
    return ok({ status: 'healthy' as const, lane: 'local', detail: 'Messages send and chat.db read access are available.' });
  }
}

export function createMessagesProvider(spec: MessagesAdapterSpec, options: MessagesProviderOptions = {}): MessagesProvider {
  return new MessagesProvider(spec, options);
}
