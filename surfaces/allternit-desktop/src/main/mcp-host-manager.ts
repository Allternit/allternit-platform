/**
 * Native MCP Host — manages Model Context Protocol servers in the Electron main process.
 *
 * Architecture: one official-SDK MCP Client per configured server, each running as a
 * lifeline-wrapped child process over stdio. The client negotiates the protocol era:
 * MCP 2026-07-28 (`server/discover`) when the server offers it, otherwise the legacy
 * `initialize` handshake. Works even when gizzi-code is down.
 *
 * Config: ~/.allternit/mcp-config.json  (Claude-compatible format)
 */

import * as fs from 'node:fs';
import * as path from 'node:path';
import * as child_process from 'node:child_process';
import { app, BrowserWindow } from 'electron';
import log from 'electron-log';
import type { Client } from '@modelcontextprotocol/client';
import { createNegotiatingClient, ProcStdioTransport } from './mcp-stdio-transport.js';
import { spawnSidecar } from './process-lifeline.js';

// ── Config shape (Claude Desktop compatible) ─────────────────────────────────

export interface McpServerConfig {
  command: string;
  args?: string[];
  env?: Record<string, string>;
  alwaysAllow?: string[];
  disabled?: boolean;
}

export interface McpConfig {
  mcpServers: Record<string, McpServerConfig>;
}

// ── Server lifecycle states ──────────────────────────────────────────────────

type ServerStatus = 'connecting' | 'running' | 'error' | 'disabled' | 'stopped';

interface ToolInfo {
  name: string;
  description?: string;
  inputSchema?: Record<string, unknown>;
}

interface ServerEntry {
  id: string;
  config: McpServerConfig;
  status: ServerStatus;
  tools: ToolInfo[];
  proc: child_process.ChildProcess | null;
  restartCount: number;
  errorMessage?: string;
  client: Client | null;
}

const MAX_RESTARTS = 3;
const RESTART_BACKOFF_BASE_MS = 1000;
const TOOL_CALL_TIMEOUT_MS = 30_000;

class McpHostManager {
  private servers = new Map<string, ServerEntry>();
  /**
   * Servers that exited while answering the `server/discover` probe (2025-era
   * servers built on SDKs that quit on any pre-`initialize` request). They are
   * restarted once with the plain legacy handshake and stay legacy until the
   * app restarts.
   */
  private legacyOnly = new Set<string>();
  private configPath: string;
  private configWatcher: fs.FSWatcher | null = null;

  constructor() {
    this.configPath = path.join(app.getPath('home'), '.allternit', 'mcp-config.json');
  }

  initialize(): void {
    const config = this.loadConfig();
    for (const [id, serverConfig] of Object.entries(config.mcpServers)) {
      if (!serverConfig.disabled) {
        this.startServer(id, serverConfig);
      } else {
        this.servers.set(id, this.makeEntry(id, serverConfig, 'disabled'));
      }
    }
    this.watchConfig();
    log.info(`[MCPHost] Initialized with ${this.servers.size} servers`);
  }

  listServers(): Array<{ id: string; status: ServerStatus; toolCount: number; error?: string }> {
    return [...this.servers.values()].map(e => ({
      id: e.id,
      status: e.status,
      toolCount: e.tools.length,
      error: e.errorMessage,
    }));
  }

  listTools(serverId?: string): Array<{ serverId: string } & ToolInfo> {
    const results: Array<{ serverId: string } & ToolInfo> = [];
    for (const [id, entry] of this.servers) {
      if (serverId && id !== serverId) continue;
      if (entry.status !== 'running') continue;
      for (const tool of entry.tools) {
        results.push({ serverId: id, ...tool });
      }
    }
    return results;
  }

  async callTool(serverId: string, toolName: string, args: unknown): Promise<unknown> {
    const entry = this.servers.get(serverId);
    if (!entry || entry.status !== 'running' || !entry.proc) {
      throw new Error(`MCP server "${serverId}" is not running`);
    }

    // Check alwaysAllow — if tool is not pre-approved, caller must have checked permission
    const config = entry.config;
    const allowed = config.alwaysAllow?.includes(toolName) ?? false;
    if (!allowed) {
      log.info(`[MCPHost] Tool call requires user approval: ${serverId}/${toolName}`);
      // Permission enforcement is handled by the IPC handler (shows dialog before calling here)
    }

    if (!entry.client) throw new Error(`MCP server "${serverId}" is not connected`);
    return entry.client.callTool(
      { name: toolName, arguments: (args ?? {}) as Record<string, unknown> },
      { timeout: TOOL_CALL_TIMEOUT_MS },
    );
  }

  async addServer(id: string, config: McpServerConfig): Promise<void> {
    if (this.servers.has(id)) {
      throw new Error(`Server "${id}" already exists`);
    }
    const fullConfig = this.loadConfig();
    fullConfig.mcpServers[id] = config;
    this.saveConfig(fullConfig);
    if (!config.disabled) this.startServer(id, config);
  }

  /**
   * Run a server owned by Desktop itself (e.g. the phone shim). Not written to
   * mcp-config.json: its command path changes with every app update, so it is
   * re-registered at each launch instead of persisted stale.
   */
  registerRuntimeServer(id: string, config: McpServerConfig): void {
    if (this.servers.has(id)) return;
    this.startServer(id, { ...config, alwaysAllow: [] });
  }

  async removeServer(id: string): Promise<void> {
    this.stopServer(id);
    const fullConfig = this.loadConfig();
    delete fullConfig.mcpServers[id];
    this.saveConfig(fullConfig);
  }

  shutdown(): void {
    this.configWatcher?.close();
    for (const id of this.servers.keys()) this.stopServer(id);
  }

  // ── Private ────────────────────────────────────────────────────────────────

  private startServer(id: string, config: McpServerConfig, restartCount = 0): void {
    const entry = this.makeEntry(id, config, 'connecting', restartCount);
    this.servers.set(id, entry);

    try {
      // MCP only says a client *should* close stdin; not every server exits
      // on EOF, so they get the same lifeline as other sidecars.
      const proc = spawnSidecar(config.command, config.args ?? [], {
        env: { ...process.env, ...(config.env ?? {}) } as NodeJS.ProcessEnv,
        stdio: ['pipe', 'pipe', 'pipe'],
        windowsHide: true,
      });

      entry.proc = proc;

      proc.stderr?.setEncoding('utf-8');
      proc.stderr?.on('data', (chunk: string) => {
        log.warn(`[MCPHost:${id}] stderr:`, chunk.trim());
      });

      proc.on('error', (err) => {
        log.error(`[MCPHost:${id}] spawn error:`, err);
        this.markError(entry, err.message);
      });

      proc.on('exit', (code) => {
        log.warn(`[MCPHost:${id}] exited (code ${code})`);
        entry.proc = null;
        entry.client = null;
        if (entry.status === 'stopped' || this.servers.get(id) !== entry) return;

        if (entry.status === 'connecting' && !this.legacyOnly.has(id)) {
          // Exited during era negotiation: retry once on the legacy handshake.
          this.legacyOnly.add(id);
          log.info(`[MCPHost:${id}] exited during server/discover probe; retrying with legacy initialize`);
          this.startServer(id, config, restartCount);
          return;
        }

        if (restartCount < MAX_RESTARTS) {
          const delay = RESTART_BACKOFF_BASE_MS * Math.pow(2, restartCount);
          log.info(`[MCPHost:${id}] Restarting in ${delay}ms (attempt ${restartCount + 1}/${MAX_RESTARTS})`);
          setTimeout(() => this.startServer(id, config, restartCount + 1), delay);
        } else {
          this.markError(entry, `Exceeded max restarts (${MAX_RESTARTS})`);
          this.broadcast('mcp:server-dead', { serverId: id });
        }
      });

      // Initialize handshake
      this.initializeServer(entry);

    } catch (err) {
      this.markError(entry, (err as Error).message);
    }
  }

  private async initializeServer(entry: ServerEntry): Promise<void> {
    const proc = entry.proc;
    if (!proc?.stdin || !proc.stdout) {
      this.markError(entry, 'Server process stdio not available');
      return;
    }
    const client = createNegotiatingClient('allternit-desktop', '1.0.0', this.legacyOnly.has(entry.id) ? 'legacy' : 'auto');
    client.onerror = (err) => log.warn(`[MCPHost:${entry.id}] ${err.message}`);
    try {
      await client.connect(
        new ProcStdioTransport({
          stdin: proc.stdin,
          stdout: proc.stdout,
          stderr: proc.stderr,
          pid: proc.pid,
          kill: () => proc.kill('SIGTERM'),
          onExit: (cb) => proc.once('exit', cb),
        }),
        { timeout: TOOL_CALL_TIMEOUT_MS },
      );
      entry.client = client;

      // Discover tools (all pages), in the server's order.
      const tools: ToolInfo[] = [];
      let cursor: string | undefined;
      for (let page = 0; page < 50; page++) {
        const result = await client.listTools(cursor ? { cursor } : undefined, { timeout: TOOL_CALL_TIMEOUT_MS });
        for (const t of result.tools) tools.push({ name: t.name, description: t.description, inputSchema: t.inputSchema as Record<string, unknown> });
        cursor = result.nextCursor;
        if (!cursor) break;
      }
      entry.tools = tools;
      entry.status = 'running';

      log.info(`[MCPHost:${entry.id}] Running with ${entry.tools.length} tools (${client.getProtocolEra() ?? 'legacy'} era, ${client.getNegotiatedProtocolVersion() ?? 'unknown'})`);
      this.broadcast('mcp:server-ready', { serverId: entry.id, tools: entry.tools });

    } catch (err) {
      if (!entry.proc) return; // exited mid-handshake; the exit handler decides what happens next
      this.markError(entry, (err as Error).message);
    }
  }

  private stopServer(id: string): void {
    const entry = this.servers.get(id);
    if (!entry) return;
    entry.status = 'stopped';
    void entry.client?.close().catch(() => undefined);
    entry.client = null;
    if (entry.proc) {
      try { entry.proc.stdin?.end(); } catch { /* ignore */ }
      setTimeout(() => { try { entry.proc?.kill('SIGTERM'); } catch { /* ignore */ } }, 500);
    }
    entry.status = 'stopped';
    entry.proc = null;
  }

  private markError(entry: ServerEntry, message: string): void {
    entry.status = 'error';
    entry.errorMessage = message;
    this.broadcast('mcp:server-error', { serverId: entry.id, error: message });
  }

  private makeEntry(id: string, config: McpServerConfig, status: ServerStatus, restartCount = 0): ServerEntry {
    return { id, config, status, tools: [], proc: null, restartCount, client: null };
  }

  private loadConfig(): McpConfig {
    try {
      if (fs.existsSync(this.configPath)) {
        return JSON.parse(fs.readFileSync(this.configPath, 'utf-8')) as McpConfig;
      }
    } catch (err) {
      log.warn('[MCPHost] Failed to load config:', err);
    }
    return { mcpServers: {} };
  }

  private saveConfig(config: McpConfig): void {
    try {
      const dir = path.dirname(this.configPath);
      if (!fs.existsSync(dir)) fs.mkdirSync(dir, { recursive: true });
      fs.writeFileSync(this.configPath, JSON.stringify(config, null, 2));
    } catch (err) {
      log.warn('[MCPHost] Failed to save config:', err);
    }
  }

  private watchConfig(): void {
    try {
      this.configWatcher = fs.watch(this.configPath, { persistent: false }, () => {
        log.info('[MCPHost] Config changed — reloading');
        const config = this.loadConfig();
        // Start any new servers
        for (const [id, cfg] of Object.entries(config.mcpServers)) {
          if (!this.servers.has(id) && !cfg.disabled) this.startServer(id, cfg);
        }
      });
    } catch { /* file may not exist yet */ }
  }

  private broadcast(channel: string, data: unknown): void {
    for (const win of BrowserWindow.getAllWindows()) {
      if (!win.isDestroyed()) win.webContents.send(channel, data);
    }
  }
}

export const mcpHostManager = new McpHostManager();
