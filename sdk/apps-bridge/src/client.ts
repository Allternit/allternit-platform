/**
 * AppsBridge: a small typed client for the standard MCP Apps bridge (JSON-RPC
 * over postMessage to the host). Dependency-free; use it from a bundled View,
 * or skip it and use the same methods directly (see the docs quickstart).
 */
import { METHODS, PROTOCOL_VERSION } from "./protocol.js";
import type { HostContext, InitializeResult, ToolInput, ToolResult } from "./protocol.js";

export interface AppsBridgeOptions {
  appInfo: { name: string; version: string };
  /** The window to talk to; defaults to `window.parent`. */
  target?: Pick<Window, "postMessage">;
  /** The window that owns the listener; defaults to `window`. */
  self?: Window;
  /** Applies `hostContext.styles.variables` to the document root. Default true. */
  applyHostStyles?: boolean;
}

type Listener<T> = (value: T) => void;

export class BridgeError extends Error {
  constructor(message: string, readonly code?: number) {
    super(message);
    this.name = "BridgeError";
  }
}

export class AppsBridge {
  hostContext: HostContext = {};
  private seq = 0;
  private pending = new Map<number, { resolve: (v: any) => void; reject: (e: Error) => void }>();
  private results = new Set<Listener<ToolResult>>();
  private inputs = new Set<Listener<ToolInput>>();
  private contexts = new Set<Listener<HostContext>>();
  private readonly self: Window;
  private readonly target: Pick<Window, "postMessage">;
  private readonly onMessage = (event: MessageEvent) => this.handle(event);

  constructor(private readonly options: AppsBridgeOptions) {
    this.self = options.self ?? window;
    this.target = options.target ?? this.self.parent;
    this.self.addEventListener("message", this.onMessage);
  }

  /** Handshake. Resolves with the host context; also sends `ui/notifications/initialized`. */
  async connect(): Promise<HostContext> {
    const result = await this.request<InitializeResult>(METHODS.initialize, {
      protocolVersion: PROTOCOL_VERSION,
      appInfo: this.options.appInfo,
      appCapabilities: {},
    });
    this.setContext(result?.hostContext ?? {});
    this.notify(METHODS.initialized, {});
    return this.hostContext;
  }

  onToolResult(fn: Listener<ToolResult>): () => void {
    this.results.add(fn);
    return () => this.results.delete(fn);
  }

  onToolInput(fn: Listener<ToolInput>): () => void {
    this.inputs.add(fn);
    return () => this.inputs.delete(fn);
  }

  onHostContextChanged(fn: Listener<HostContext>): () => void {
    this.contexts.add(fn);
    return () => this.contexts.delete(fn);
  }

  /** Call a tool on the app's own server through the host. */
  callTool(name: string, args: Record<string, unknown> = {}): Promise<ToolResult> {
    return this.request<ToolResult>(METHODS.callTool, { name, arguments: args });
  }

  openLink(url: string): Promise<unknown> {
    return this.request(METHODS.openLink, { url });
  }

  /** Add a message to the conversation as the user. */
  sendMessage(text: string): Promise<unknown> {
    return this.request(METHODS.message, { role: "user", content: [{ type: "text", text }] });
  }

  updateModelContext(params: { content?: ToolResult["content"]; structuredContent?: Record<string, unknown> }): Promise<unknown> {
    return this.request(METHODS.updateModelContext, params);
  }

  sendSize(height = this.self.document.documentElement.scrollHeight): void {
    this.notify(METHODS.sizeChanged, { height });
  }

  dispose(): void {
    this.self.removeEventListener("message", this.onMessage);
    for (const p of this.pending.values()) p.reject(new BridgeError("bridge disposed"));
    this.pending.clear();
  }

  request<T = unknown>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    const id = ++this.seq;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.target.postMessage({ jsonrpc: "2.0", id, method, params }, "*");
    });
  }

  notify(method: string, params: Record<string, unknown> = {}): void {
    this.target.postMessage({ jsonrpc: "2.0", method, params }, "*");
  }

  private setContext(next: HostContext): void {
    this.hostContext = { ...this.hostContext, ...next };
    if (this.options.applyHostStyles !== false) {
      const vars = next.styles?.variables ?? {};
      for (const [k, v] of Object.entries(vars)) {
        if (v && k.startsWith("--")) this.self.document.documentElement.style.setProperty(k, v);
      }
    }
  }

  private handle(event: MessageEvent): void {
    if (event.source !== this.target) return;
    const msg = event.data;
    if (!msg || msg.jsonrpc !== "2.0") return;
    if (msg.id !== undefined && msg.method === undefined) {
      const p = this.pending.get(msg.id);
      if (!p) return;
      this.pending.delete(msg.id);
      if (msg.error) p.reject(new BridgeError(msg.error.message ?? "request failed", msg.error.code));
      else p.resolve(msg.result);
      return;
    }
    switch (msg.method) {
      case METHODS.toolResult:
        this.results.forEach((fn) => fn(msg.params as ToolResult));
        break;
      case METHODS.toolInput:
        this.inputs.forEach((fn) => fn(msg.params as ToolInput));
        break;
      case METHODS.hostContextChanged:
        this.setContext((msg.params ?? {}) as HostContext);
        this.contexts.forEach((fn) => fn(this.hostContext));
        break;
      case METHODS.teardown:
        if (msg.id !== undefined) this.target.postMessage({ jsonrpc: "2.0", id: msg.id, result: {} }, "*");
        break;
    }
  }
}
