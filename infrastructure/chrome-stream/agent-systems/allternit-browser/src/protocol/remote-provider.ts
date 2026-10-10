import { spawn, type ChildProcess } from 'node:child_process';
import { existsSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  BrowserEventSchema,
  BrowserObservationSchema,
  COMPUTER_USE_PROTOCOL_VERSION,
  type ActionIntent,
  type BrowserEvent,
  type BrowserObservation,
  type BrowserProvider,
  type ProviderCapabilities,
  type ProviderKind,
} from '@allternit/computer-use-protocol';

export interface RemoteBrowserProviderOptions {
  capabilities: ProviderCapabilities;
  baseUrl: string;
  token?: string;
  fetchImpl?: FetchLike;
}

export type FetchLike = (input: URL, init: RequestInit) => Promise<Response>;

export class RemoteBrowserProvider implements BrowserProvider {
  readonly capabilities: ProviderCapabilities;
  private readonly baseUrl: URL;
  private readonly token?: string;
  private readonly fetchImpl: FetchLike;

  constructor(options: RemoteBrowserProviderOptions) {
    this.capabilities = options.capabilities;
    this.baseUrl = new URL(options.baseUrl);
    this.token = options.token;
    this.fetchImpl = options.fetchImpl ?? ((input, init) => fetch(input, init));
  }

  async observe(sessionId: string): Promise<BrowserObservation> {
    const payload = await this.request('POST', '/v1/browser/observe', { sessionId });
    return BrowserObservationSchema.parse(payload);
  }

  async execute(action: ActionIntent): Promise<BrowserEvent[]> {
    const payload = await this.request('POST', '/v1/browser/actions', { action });
    if (!Array.isArray(payload)) throw new Error('Remote browser provider returned a non-array action response');
    return payload.map((event) => BrowserEventSchema.parse(event));
  }

  async close(sessionId: string): Promise<void> {
    await this.request('POST', '/v1/browser/close', { sessionId });
  }

  private async request(method: string, path: string, body: unknown): Promise<unknown> {
    const response = await this.fetchImpl(new URL(path, this.baseUrl), {
      method,
      headers: {
        'content-type': 'application/json',
        ...(this.token ? { authorization: `Bearer ${this.token}` } : {}),
      },
      body: JSON.stringify(body),
    });
    if (!response.ok) {
      throw new Error(`Remote browser provider ${this.capabilities.provider} failed ${path}: ${response.status}`);
    }
    if (response.status === 204) return undefined;
    return response.json();
  }
}

export function createStagehandProvider(
  options: Omit<RemoteBrowserProviderOptions, 'capabilities'> & StagehandSidecarOptions = {
    baseUrl: 'sidecar://local',
  },
): BrowserProvider {
  return new StagehandSidecarProvider({
    ...options,
    capabilities: {
      provider: 'stagehand',
      capabilities: [
        'navigate', 'observe.dom', 'observe.accessibility', 'observe.screenshot',
        'interact.pointer', 'interact.keyboard', 'tabs', 'network.inspect', 'console.inspect',
        'files.upload', 'files.download', 'dialogs',
      ],
      local: true,
      attachedToUserSession: false,
      supportsPrivateNetwork: true,
      supportsPersistentProfile: true,
      limits: { maxRunMs: 30 * 60_000 },
    },
  });
}

export interface StagehandSidecarOptions {
  /**
   * Path to the allternit-browser-runtime package root (the vendored fork at
   * infrastructure/chrome-stream/agent-systems/allternit-browser-runtime).
   * Defaults to ALLTERNIT_BROWSER_RUNTIME_DIR, then an upward search from this
   * module and process.cwd() for the runtime's sidecar entrypoint.
   */
  runtimeDir?: string;
  /** Node ≥ 22.18 executable used to run the sidecar (the runtime requires it). */
  nodeExecutable?: string;
  /** Launch the sidecar's Chrome headless (default true). */
  headless?: boolean;
  /**
   * Model mode for the sidecar runtime. P0 default 'mock' (canned
   * structured outputs — no external inference). 'gateway' routes the
   * client-LLM callback through the allternit gateway
   * (POST {ALLTERNIT_GATEWAY_URL}/v1/chat/completions with the
   * ALLTERNIT_GATEWAY_KEY Bearer virtual key, model from
   * ALLTERNIT_BROWSER_RUNTIME_MODEL or the A://C routing default).
   * Fail-closed: no direct provider keys are used anywhere.
   */
  modelMode?: 'mock' | 'gateway';
  /**
   * Run-scoped sandbox directory for file intents: downloads land in
   * `<sandboxDir>/downloads`, path-based uploads are containment-checked
   * against this root. Defaults to
   * `<os.tmpdir()>/allternit-browser-runs/<first-run-id>` so different runs
   * never share a download/upload surface.
   */
  sandboxDir?: string;
}

type SidecarRequest = { id: number; method: string; params?: Record<string, unknown> };
type SidecarResponse = { id: number; ok: boolean; result?: unknown; error?: string };

const SIDECAR_ENTRY = 'packages/sdk-ts/sidecar/allternit-browser-runtime-sidecar.mjs';

function resolveRuntimeDir(explicit?: string): string {
  const candidates = [explicit, process.env.ALLTERNIT_BROWSER_RUNTIME_DIR].filter(
    (value): value is string => typeof value === 'string' && value.length > 0,
  );
  const starts = [path.dirname(fileURLToPath(import.meta.url)), process.cwd()];
  for (const start of starts) {
    let dir = start;
    for (;;) {
      candidates.push(dir);
      const parent = path.dirname(dir);
      if (parent === dir) break;
      dir = parent;
    }
  }
  for (const candidate of candidates) {
    if (existsSync(path.join(candidate, SIDECAR_ENTRY))) return candidate;
  }
  throw new Error(
    'allternit-browser-runtime not found. Set ALLTERNIT_BROWSER_RUNTIME_DIR or pass runtimeDir.',
  );
}

/**
 * BrowserProvider backed by the vendored @allternit/browser-runtime running in
 * a child-process sidecar (stdio NDJSON). The sidecar hosts the Node-≥22
 * toolchain, keeping this package's Node-18 toolchain separate — nothing from
 * the runtime is imported in-process. The sidecar (and its Chrome) spawn
 * lazily on the first execute()/observe() call.
 */
export class StagehandSidecarProvider implements BrowserProvider {
  readonly capabilities: ProviderCapabilities;
  private readonly options: StagehandSidecarOptions;
  private child: ChildProcess | undefined;
  private nextRequestId = 1;
  private sequence = 0;
  private sessionId: string | undefined;
  private pending = new Map<number, { resolve: (value: unknown) => void; reject: (error: Error) => void }>();
  private initPromise: Promise<unknown> | undefined;
  private stdioBuffer = '';

  constructor(options: { capabilities: ProviderCapabilities } & StagehandSidecarOptions) {
    this.capabilities = options.capabilities;
    this.options = options;
  }

  async observe(sessionId: string): Promise<BrowserObservation> {
    this.sessionId = sessionId;
    await this.ensureReady(undefined);
    const [actionsResult, pageInfo] = await Promise.all([
      this.request<{ actions: Array<{ selector: string; description: string; method?: string }> }>(
        'observe',
        { instruction: 'find every interactive element on the page' },
      ),
      this.request<{ url: string; title: string }>('pageInfo', {}),
    ]);
    const actions = actionsResult.actions ?? [];
    return BrowserObservationSchema.parse({
      schemaVersion: COMPUTER_USE_PROTOCOL_VERSION,
      observationId: randomId('obs'),
      sessionId,
      url: pageInfo.url,
      title: pageInfo.title,
      capturedAt: new Date().toISOString(),
      format: 'accessibility',
      text: actions.map((action) => action.description).join('\n'),
      refs: actions.map((action, index) => ({
        ref: action.selector || `action-${index}`,
        role: 'element',
        name: action.description,
        selector: action.selector,
      })),
    });
  }

  async execute(action: ActionIntent): Promise<BrowserEvent[]> {
    this.sessionId = action.sessionId;
    try {
      await this.ensureReady(action.runId);
      const result = await this.executeKind(action);
      return [this.event(action, 'committed', result)];
    } catch (error) {
      return [
        this.event(action, 'failed', {
          error: error instanceof Error ? error.message : String(error),
        }),
      ];
    }
  }

  async close(sessionId: string): Promise<void> {
    if (this.child && this.sessionId === sessionId) {
      await this.request('close', {}).catch(() => undefined);
      this.child.kill('SIGTERM');
      this.child = undefined;
      this.initPromise = undefined;
      this.sessionId = undefined;
    }
  }

  private async executeKind(action: ActionIntent): Promise<Record<string, unknown>> {
    const target = action.targetDescription ?? action.targetRef ?? '';
    switch (action.kind) {
      case 'navigate':
        return await this.request('navigate', { url: String(action.input.url ?? '') });
      case 'click':
        return await this.request('act', { instruction: `click ${target}` });
      case 'type':
        return await this.request('act', {
          instruction: `type "${String(action.input.text ?? '')}" into ${target}`,
        });
      case 'press':
        return await this.request('act', { instruction: `press the ${String(action.input.key ?? '')} key` });
      case 'scroll':
        return await this.request('act', { instruction: `scroll ${target}` });
      case 'select':
        return await this.request('act', { instruction: `select ${String(action.input.value ?? '')} in ${target}` });
      case 'hover':
        return await this.request('act', { instruction: `hover over ${target}` });
      case 'extract':
        return await this.request('extract', {
          instruction: String(action.input.instruction ?? action.reason),
          schema: action.input.schema,
        });
      case 'screenshot': {
        const result = await this.request<{ pngBase64: string; sha256?: string }>('screenshot', {});
        // The runtime hashes the PNG at capture time; carry the hash in the
        // event payload so receipt metadata binds the exact pixels observed.
        return { ...result, sha256: result.sha256 ?? null };
      }
      case 'wait':
        await new Promise((resolve) => setTimeout(resolve, Number(action.input.ms ?? 500)));
        return { waitedMs: Number(action.input.ms ?? 500) };
      // Tabs ride the vendored SDK's BrowserContext (pageId === CDP targetId).
      case 'tab.open':
        return await this.request('tabOpen', { url: action.input.url });
      case 'tab.focus':
        return await this.request('tabSwitch', {
          pageId: String(action.input.pageId ?? action.targetRef ?? ''),
        });
      case 'tab.close':
        return await this.request('tabClose', {
          pageId: String(action.input.pageId ?? action.targetRef ?? ''),
        });
      // Dialogs are handled host-side over raw CDP (the extension protocol
      // has no dialog op — nothing invented that the extension can't run).
      case 'dialog.accept':
        return await this.request('dialog', {
          accept: true,
          promptText: action.input.promptText,
          timeoutMs: action.input.timeoutMs,
        });
      case 'dialog.dismiss':
        return await this.request('dialog', {
          accept: false,
          timeoutMs: action.input.timeoutMs,
        });
      // Files: uploads are containment-checked server-side against the
      // run-scoped sandbox dir; downloads list the sandbox downloads dir.
      case 'file.upload':
        return await this.request('upload', {
          selector: String(action.input.selector ?? target),
          files: action.input.files,
        });
      case 'download':
        return await this.request('downloads', {});
      default:
        throw new Error(`action kind "${action.kind}" is not supported by the stagehand sidecar provider`);
    }
  }

  private event(action: ActionIntent, state: string, payload: Record<string, unknown>): BrowserEvent {
    return BrowserEventSchema.parse({
      schemaVersion: COMPUTER_USE_PROTOCOL_VERSION,
      eventId: randomId('evt'),
      runId: action.runId,
      sessionId: action.sessionId,
      sequence: ++this.sequence,
      emittedAt: new Date().toISOString(),
      type: 'action.state_changed',
      payload: { actionId: action.actionId, state, ...payload },
    });
  }

  private async ensureReady(runId?: string): Promise<void> {
    if (!this.child) this.spawn();
    this.initPromise ??= (async () => {
      const sandboxDir = this.resolveSandboxDir(runId);
      await this.request('init', {
        headless: this.options.headless ?? true,
        model: this.options.modelMode === 'gateway' ? { mode: 'gateway' } : { mode: 'mock' },
        // Run-scoped sandbox: downloads are pinned here at launch
        // (Browser.setDownloadBehavior) and path-based uploads are
        // containment-checked against this root by the sidecar.
        ...(sandboxDir
          ? { sandboxDir, downloadsPath: path.join(sandboxDir, 'downloads') }
          : {}),
      });
    })();
    await this.initPromise;
  }

  private resolveSandboxDir(runId?: string): string | undefined {
    if (this.options.sandboxDir) return this.options.sandboxDir;
    if (!runId) return undefined;
    return path.join(os.tmpdir(), 'allternit-browser-runs', sanitizeRunId(runId));
  }

  private spawn(): void {
    const runtimeDir = resolveRuntimeDir(this.options.runtimeDir);
    const node = this.options.nodeExecutable ?? process.env.ALLTERNIT_BROWSER_RUNTIME_NODE ?? 'node';
    this.child = spawn(node, [path.join(runtimeDir, SIDECAR_ENTRY)], { stdio: ['pipe', 'pipe', 'inherit'] });
    this.child.stdout?.setEncoding('utf8');
    this.child.stdout?.on('data', (chunk: string) => {
      this.stdioBuffer += chunk;
      let newline: number;
      while ((newline = this.stdioBuffer.indexOf('\n')) >= 0) {
        const line = this.stdioBuffer.slice(0, newline);
        this.stdioBuffer = this.stdioBuffer.slice(newline + 1);
        if (!line.trim()) continue;
        let response: SidecarResponse;
        try {
          response = JSON.parse(line);
        } catch {
          continue;
        }
        const waiter = this.pending.get(response.id);
        if (!waiter) continue;
        this.pending.delete(response.id);
        if (response.ok) waiter.resolve(response.result);
        else waiter.reject(new Error(response.error ?? 'sidecar error'));
      }
    });
    this.child.on('exit', (code) => {
      const error = new Error(`allternit-browser-runtime sidecar exited (code ${code})`);
      for (const waiter of this.pending.values()) waiter.reject(error);
      this.pending.clear();
      this.child = undefined;
      this.initPromise = undefined;
    });
  }

  private request<Result = Record<string, unknown>>(
    method: string,
    params: Record<string, unknown>,
  ): Promise<Result> {
    if (!this.child?.stdin) return Promise.reject(new Error('stagehand sidecar is not running'));
    const id = this.nextRequestId++;
    return new Promise<Result>((resolve, reject) => {
      this.pending.set(id, { resolve: resolve as (value: unknown) => void, reject });
      this.child!.stdin!.write(JSON.stringify({ id, method, params } satisfies SidecarRequest) + '\n');
    });
  }
}

function randomId(prefix: string): string {
  return `${prefix}-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
}

/** Run ids become directory names; keep only safe characters. */
function sanitizeRunId(runId: string): string {
  const sanitized = runId.replace(/[^a-zA-Z0-9._-]/g, '_');
  return sanitized.slice(0, 64) || 'run';
}

export interface ExtensionTabProviderOptions {
  send: (message: ExtensionTabProviderMessage) => Promise<unknown>;
}

export type ExtensionTabProviderMessage =
  | { type: 'browser.observe'; sessionId: string }
  | { type: 'browser.execute'; action: ActionIntent }
  | { type: 'browser.close'; sessionId: string };

export class ExtensionTabProvider implements BrowserProvider {
  readonly capabilities: ProviderCapabilities = {
    provider: 'extension-tab',
    capabilities: [
      'navigate', 'observe.dom', 'observe.accessibility', 'observe.screenshot',
      'interact.pointer', 'interact.keyboard', 'tabs', 'files.upload', 'files.download',
    ],
    local: true,
    attachedToUserSession: true,
    supportsPrivateNetwork: true,
    supportsPersistentProfile: true,
    limits: { maxTabs: 1, maxObservationChars: 24_000 },
  };

  private readonly send: ExtensionTabProviderOptions['send'];

  constructor(options: ExtensionTabProviderOptions) {
    this.send = options.send;
  }

  async observe(sessionId: string): Promise<BrowserObservation> {
    const payload = await this.send({ type: 'browser.observe', sessionId });
    return BrowserObservationSchema.parse(payload);
  }

  async execute(action: ActionIntent): Promise<BrowserEvent[]> {
    const payload = await this.send({ type: 'browser.execute', action });
    if (!Array.isArray(payload)) throw new Error('Extension tab provider returned a non-array action response');
    return payload.map((event) => BrowserEventSchema.parse(event));
  }

  async close(sessionId: string): Promise<void> {
    await this.send({ type: 'browser.close', sessionId });
  }
}

export function providerKind(provider: BrowserProvider): ProviderKind {
  return provider.capabilities.provider;
}
