/**
 * Phone run history + phone skills, Desktop side.
 *
 * - PhoneRunRecorder: after every phone tool call, posts one step to the
 *   local allternit-api (POST /api/v1/phones/:serial/history). Fire-and-forget:
 *   it never blocks or fails a tool call. Typed text, SMS bodies and numbers
 *   are redacted here (and again by the API). The last screen read per phone
 *   is kept so a tap carries the element it landed on — that is what lets a
 *   saved skill find the element again (cmd/allternit-api/src/phone_replay.rs).
 * - PhoneApi: the allternit-api phone endpoints the bot tools call
 *   (phone.task on the bot's brain, phone.skills / run_skill / save_skill).
 *
 * Electron-free so it is unit-testable; phone-service.ts wires the HTTP
 * client (URLS.API + backendManager.getLocalAuthHeaders()) and the frame
 * shrinker (nativeImage).
 */

/** A run that saw no call for this long ends; the next call starts a new one. */
export const RUN_IDLE_MS = 5 * 60_000;
/** Screen outline posted per step (the API keeps 24 KB). */
const MAX_UI_CHARS = 24_000;
const REDACT_KEYS = new Set(['text', 'to', 'number', 'body', 'message']);
/** Reads that don't change the screen (no target to attach). */
const READ_TOOLS = new Set(['phone.devices', 'phone.screenshot', 'phone.read_ui', 'phone.screen_text', 'phone.skills', 'phone.maps']);

export interface PhoneStepIn {
  run_id: string;
  tool: string;
  args: Record<string, unknown>;
  ok: boolean;
  error?: string;
  duration_ms: number;
  ui?: string;
  target?: { id: string; label: string; class: string; rect: [number, number, number, number]; screen: [number, number] };
  source?: string;
  /** `map` when an App Map replay made the call (allternit-api's `go_to`), with the screen it went to. */
  via?: 'map';
  map_screen?: string;
  image_base64?: string;
  at: string;
}

export interface PhoneTaskRequest {
  goal: string;
  max_steps?: number;
  bot_id?: string;
  brain?: string;
  /** Screenshot policy: "auto" skips it when the accessibility tree is rich; "always" sends it every step. */
  vision?: 'auto' | 'always';
}

export interface PhoneExploreRequest {
  serial: string;
  minutes?: number;
  max_screens?: number;
}

/** The allternit-api phone endpoints. Each throws PhoneApiUnavailable when the API can't be reached. */
export interface PhoneApi {
  postStep(serial: string, step: PhoneStepIn): Promise<void>;
  task(serial: string, body: PhoneTaskRequest): Promise<Record<string, unknown>>;
  skills(): Promise<Record<string, unknown>>;
  saveSkill(body: Record<string, unknown>): Promise<Record<string, unknown>>;
  runSkill(name: string, body: Record<string, unknown>): Promise<Record<string, unknown>>;
  /** App maps (contract phone-app-map §4). Errors come back as { ok:false, error:<contract code>, message, status }. */
  maps(platform?: string): Promise<Record<string, unknown>>;
  map(app: string, platform?: string): Promise<Record<string, unknown>>;
  forgetMap(app: string, platform?: string): Promise<Record<string, unknown>>;
  explore(app: string, body: PhoneExploreRequest): Promise<Record<string, unknown>>;
  /** POST /api/aci/stop/:id: stops a task, skill or Explore run. */
  stopRun(runId: string): Promise<Record<string, unknown>>;
}

export class PhoneApiUnavailable extends Error {}

/** The shared screen-read text form: `class "label" [id] {{x, y}, {w, h}}`. */
export interface UiLine {
  className: string;
  label: string;
  id: string;
  rect: { x: number; y: number; w: number; h: number };
}

export function parseUiLine(line: string): UiLine | null {
  const m = /^(\S+)(?: "(.*)")?(?: \[([^\]]*)\])? \{\{(-?[\d.]+), (-?[\d.]+)\}, \{(-?[\d.]+), (-?[\d.]+)\}\}$/.exec(line.trim());
  if (!m) return null;
  return { className: m[1], label: m[2] ?? '', id: m[3] ?? '', rect: { x: Number(m[4]), y: Number(m[5]), w: Number(m[6]), h: Number(m[7]) } };
}

export function parseUiText(text: string): UiLine[] {
  return text.split('\n').map(parseUiLine).filter((e): e is UiLine => e !== null);
}

/** The smallest element containing a point (what a tap landed on). */
export function elementAt(els: UiLine[], x: number, y: number): UiLine | null {
  let best: UiLine | null = null;
  for (const e of els) {
    const { rect: r } = e;
    if (x < r.x || y < r.y || x > r.x + r.w || y > r.y + r.h) continue;
    if (!best || r.w * r.h < best.rect.w * best.rect.h) best = e;
  }
  return best;
}

export function redactArgs(tool: string, args: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(args)) {
    if (k.startsWith('_')) continue;
    const sensitive = REDACT_KEYS.has(k) || (tool.includes('sms') && k !== 'serial');
    if (typeof v === 'string' && sensitive && !/^\{\{[^}]+\}\}$/.test(v)) out[k] = { redacted: true, len: v.length };
    else if (typeof v === 'string') out[k] = v.slice(0, 200);
    else if (typeof v === 'number' || typeof v === 'boolean' || v === null) out[k] = v;
    else if (sensitive) out[k] = { redacted: true };
    else if (k === 'filter' || k === 'inputs') out[k] = k === 'inputs' ? Object.keys((v as object) ?? {}) : v;
  }
  return out;
}

export interface RecorderDeps {
  api: () => PhoneApi | null;
  now?: () => number;
  /** Downscale a frame before it is posted (nativeImage in Desktop). null = drop the frame. */
  shrink?: (dataBase64: string, mimeType: string) => Promise<string | null>;
  onError?: (error: unknown) => void;
}

export class PhoneRunRecorder {
  private readonly now: () => number;
  private readonly runs = new Map<string, { id: string; last: number }>();
  private readonly screens = new Map<string, string>();

  constructor(private readonly deps: RecorderDeps) {
    this.now = deps.now ?? Date.now;
  }

  /** The run a call belongs to: `_run_id` when the caller set one, else this phone's current run. */
  runId(serial: string, explicit?: unknown): string {
    if (typeof explicit === 'string' && explicit.trim()) return explicit.trim().slice(0, 120);
    const t = this.now();
    const cur = this.runs.get(serial);
    if (cur && t - cur.last < RUN_IDLE_MS) {
      cur.last = t;
      return cur.id;
    }
    const id = `run-${new Date(t).toISOString().replace(/[-:]/g, '').replace(/\..*$/, '')}-${Math.random().toString(36).slice(2, 8)}`;
    this.runs.set(serial, { id, last: t });
    return id;
  }

  /** Record one call. Never throws, never awaited by the tool. */
  record(call: { serial: string; tool: string; args: Record<string, unknown>; result: Record<string, unknown>; durationMs: number }): void {
    void this.post(call).catch((error) => this.deps.onError?.(error));
  }

  private async post({ serial, tool, args, result, durationMs }: { serial: string; tool: string; args: Record<string, unknown>; result: Record<string, unknown>; durationMs: number }): Promise<void> {
    const api = this.deps.api();
    if (!api) return;
    const ok = result.ok === true;
    const before = this.screens.get(serial);
    const step: PhoneStepIn = {
      run_id: this.runId(serial, args._run_id),
      tool,
      args: redactArgs(tool, args),
      ok,
      duration_ms: Math.max(0, Math.round(durationMs)),
      source: typeof args._source === 'string' ? args._source.slice(0, 20) : 'bot',
      at: new Date(this.now()).toISOString(),
    };
    // Only the map marker is kept from the underscored args; `_via` and `_map_screen` never reach the phone (the runner drops every `_` arg).
    if (args._via === 'map') {
      step.via = 'map';
      if (typeof args._map_screen === 'string' && args._map_screen.trim()) step.map_screen = args._map_screen.trim().slice(0, 80);
    }
    if (!ok && typeof result.message === 'string') step.error = result.message.slice(0, 400);
    if (tool === 'phone.read_ui' && ok && typeof result.text === 'string') {
      // A filtered read is partial: keep it for this step, not as the screen.
      if (args.filter === undefined) this.screens.set(serial, result.text);
      step.ui = result.text.slice(0, MAX_UI_CHARS);
    } else if (before && !READ_TOOLS.has(tool)) {
      step.ui = before.slice(0, MAX_UI_CHARS);
      const els = parseUiText(before);
      const screen: [number, number] = [Math.max(1, ...els.map((e) => e.rect.x + e.rect.w)), Math.max(1, ...els.map((e) => e.rect.y + e.rect.h))];
      let hit: UiLine | null = null;
      if (tool === 'phone.tap' && typeof args.x === 'number' && typeof args.y === 'number') hit = elementAt(els, args.x, args.y);
      if (tool === 'phone.find_and_tap' && typeof args.element_id === 'string') {
        const id = args.element_id.toLowerCase();
        hit = els.find((e) => e.id.toLowerCase() === id || e.id.toLowerCase().endsWith(`/${id}`) || e.label.toLowerCase() === id) ?? null;
      }
      if (hit) step.target = { id: hit.id, label: hit.label, class: hit.className, rect: [hit.rect.x, hit.rect.y, hit.rect.w, hit.rect.h], screen };
    }
    // Anything that changed the screen makes the last read stale.
    if (ok && !READ_TOOLS.has(tool)) this.screens.delete(serial);
    if (ok && typeof result.dataBase64 === 'string' && typeof result.mimeType === 'string' && result.mimeType.startsWith('image/')) {
      const shrunk = this.deps.shrink ? await this.deps.shrink(result.dataBase64, result.mimeType).catch(() => null) : result.dataBase64;
      if (shrunk) step.image_base64 = shrunk;
    }
    await api.postStep(serial, step);
  }
}

/** HTTP client for the local allternit-api. */
export function httpPhoneApi(base: () => string, headers: () => Record<string, string>, fetchImpl: typeof fetch = fetch): PhoneApi {
  const req = async (method: string, path: string, body?: unknown, timeoutMs = 30_000, prefix = '/api/v1'): Promise<Record<string, unknown>> => {
    const isMaps = path.startsWith('/phones/maps');
    let res: Response;
    try {
      res = await fetchImpl(`${base().replace(/\/$/, '')}${prefix}${path}`, {
        method,
        headers: { ...headers(), 'content-type': 'application/json' },
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: AbortSignal.timeout(timeoutMs),
      });
    } catch (error) {
      throw new PhoneApiUnavailable(error instanceof Error ? error.message : String(error));
    }
    // An older API without these routes, or one without the phone gateway file.
    // Maps routes answer with contract error codes (map_not_found, phone_busy, money_app…): a 404 or 502 with a code is an answer, not a missing route.
    if (isMaps && !res.ok) {
      const j = (await res.json().catch(() => ({}))) as Record<string, unknown>;
      if (typeof j.error === 'string') return { ok: false, error: j.error, message: typeof j.message === 'string' ? j.message : `allternit-api answered ${res.status}`, status: res.status };
      if (res.status === 404) throw new PhoneApiUnavailable(`allternit-api has no ${path}`);
      throw new PhoneApiUnavailable(`allternit-api answered ${res.status}`);
    }
    if (res.status === 404 && !path.includes('/skills/')) throw new PhoneApiUnavailable(`allternit-api has no ${path}`);
    if (res.status === 503 || res.status === 502 || res.status === 401) throw new PhoneApiUnavailable(`allternit-api answered ${res.status}`);
    const json = (await res.json().catch(() => ({}))) as Record<string, unknown>;
    if (!res.ok) return { ok: false, error: res.status === 404 ? 'not_found' : 'invalid_args', message: typeof json.message === 'string' ? json.message : `allternit-api answered ${res.status}` };
    return json;
  };
  const enc = encodeURIComponent;
  return {
    postStep: async (serial, step) => {
      await req('POST', `/phones/${enc(serial)}/history`, step, 15_000);
    },
    task: (serial, body) => req('POST', `/phones/${enc(serial)}/task`, body, 6 * 60_000),
    skills: () => req('GET', '/phones/skills'),
    saveSkill: (body) => req('POST', '/phones/skills', body),
    runSkill: (name, body) => req('POST', `/phones/skills/${enc(name)}/run`, body, 10 * 60_000),
    maps: (platform) => req('GET', `/phones/maps${platform ? `?platform=${enc(platform)}` : ''}`),
    map: (app, platform) => req('GET', `/phones/maps/${enc(app)}${platform ? `?platform=${enc(platform)}` : ''}`),
    forgetMap: (app, platform) => req('DELETE', `/phones/maps/${enc(app)}${platform ? `?platform=${enc(platform)}` : ''}`),
    explore: (app, body) => req('POST', `/phones/maps/${enc(app)}/explore`, body, 30_000),
    stopRun: (runId) => req('POST', `/aci/stop/${enc(runId)}`, {}, 15_000, '/api'),
  };
}

export type PhoneMapsOp = 'list' | 'get' | 'forget' | 'explore' | 'stop';

/**
 * The renderer's app-map calls (IPC phone:maps-list|get|forget|explore|stop).
 * Validates, calls allternit-api, and returns { ok:true, … } or { ok:false, error, message }:
 * contract codes (money_app, phone_busy…) pass through; an old or stopped API is `unsupported`.
 */
export async function phoneMapsCall(api: PhoneApi | null, op: PhoneMapsOp, args: Record<string, unknown>): Promise<Record<string, unknown>> {
  const bad = (message: string) => ({ ok: false, error: 'invalid_args', message });
  const text = (v: unknown, max = 200) => (typeof v === 'string' && v.trim() && v.length <= max ? v.trim() : null);
  const platform = args.platform === 'android' || args.platform === 'ios' ? args.platform : undefined;
  if (args.platform !== undefined && !platform) return bad('platform must be android or ios.');
  const app = text(args.app);
  const runId = text(args.run_id, 120);
  if ((op === 'get' || op === 'forget' || op === 'explore') && !app) return bad('app is required.');
  if (op === 'stop' && !runId) return bad('run_id is required.');
  if (op === 'explore') {
    const serial = text(args.serial, 128);
    if (!serial) return bad('serial is required.');
    for (const [k, max] of [['minutes', 30], ['max_screens', 300]] as const) {
      const v = args[k];
      if (v !== undefined && !(Number.isInteger(v) && (v as number) >= 1 && (v as number) <= max)) return bad(`${k} must be 1–${max}.`);
    }
  }
  if (!api) return { ok: false, error: 'unsupported', message: "App maps live in Allternit Desktop's local API, which isn't running." };
  try {
    let out: Record<string, unknown>;
    if (op === 'list') out = await api.maps(platform);
    else if (op === 'get') out = await api.map(app!, platform);
    else if (op === 'forget') out = await api.forgetMap(app!, platform);
    else if (op === 'stop') out = await api.stopRun(runId!);
    else out = await api.explore(app!, { serial: String(args.serial).trim(), minutes: args.minutes as number | undefined, max_screens: args.max_screens as number | undefined });
    return out.ok === false ? out : { ...out, ok: true };
  } catch (error) {
    if (error instanceof PhoneApiUnavailable) return { ok: false, error: 'unsupported', message: `App maps need an up-to-date Allternit Desktop local API (${error.message}).` };
    throw error;
  }
}
