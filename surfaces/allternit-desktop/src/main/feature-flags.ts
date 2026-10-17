/**
 * Feature Flag System — runtime toggles for the desktop shell.
 *
 * Priority (lowest → highest):
 *   compiled defaults → config file (~/.allternit/flags.json) → env vars
 *
 * Hot-reload: editing flags.json while the app is running pushes updates
 * to the renderer via IPC.
 */

import * as fs from 'node:fs';
import * as path from 'node:path';
import { app } from 'electron';
import log from 'electron-log';

export type FlagValue = boolean | string | number;

interface FlagDefinition {
  type: 'boolean' | 'string' | 'number';
  default: FlagValue;
}

// Central registry — add new flags here.
export const FLAG_DEFINITIONS: Record<string, FlagDefinition> = {
  'devtools.auto-open':    { type: 'boolean', default: false },
  'devtools.log-renderer': { type: 'boolean', default: true  },
  'mcp.enabled':           { type: 'boolean', default: false },
  'workers.enabled':       { type: 'boolean', default: false },
  'tunnel.auto-enable':    { type: 'boolean', default: false },
  'feature.hyperframes':   { type: 'boolean', default: false },
  'feature.view-modes':    { type: 'boolean', default: true  },
  'feature.workspace-dnd': { type: 'boolean', default: true  },
};

type ChangeHandler = (key: string, value: FlagValue) => void;

class FeatureFlagManager {
  private flags: Record<string, FlagValue> = {};
  private handlers: ChangeHandler[] = [];
  private watcher: fs.FSWatcher | null = null;

  get configPath(): string {
    return path.join(app.getPath('home'), '.allternit', 'flags.json');
  }

  initialize(): void {
    this.load();
    this.startWatcher();
    log.info('[FeatureFlags] Initialized:', this.flags);
  }

  get<T extends FlagValue>(key: string): T {
    if (key in this.flags) return this.flags[key] as T;
    return (FLAG_DEFINITIONS[key]?.default ?? false) as T;
  }

  set(key: string, value: FlagValue): void {
    const prev = this.flags[key];
    this.flags[key] = value;
    this.persist();
    if (prev !== value) this.emit(key, value);
  }

  getAll(): Record<string, FlagValue> {
    const result: Record<string, FlagValue> = {};
    for (const key of Object.keys(FLAG_DEFINITIONS)) {
      result[key] = this.get(key);
    }
    return result;
  }

  onChange(handler: ChangeHandler): () => void {
    this.handlers.push(handler);
    return () => { this.handlers = this.handlers.filter(h => h !== handler); };
  }

  destroy(): void {
    this.watcher?.close();
    this.watcher = null;
  }

  // ── Private ──────────────────────────────────────────────────────────────────

  private load(): void {
    // Compiled defaults
    for (const [key, def] of Object.entries(FLAG_DEFINITIONS)) {
      this.flags[key] = def.default;
    }

    // Config file overrides
    try {
      if (fs.existsSync(this.configPath)) {
        const parsed = JSON.parse(fs.readFileSync(this.configPath, 'utf-8')) as Record<string, unknown>;
        for (const [key, raw] of Object.entries(parsed)) {
          if (key in FLAG_DEFINITIONS) {
            this.flags[key] = this.coerce(key, String(raw));
          }
        }
      }
    } catch (err) {
      log.warn('[FeatureFlags] Failed to load config file:', err);
    }

    // Env var overrides: ALLTERNIT_FLAG_<KEY_UPPER_UNDERSCORED>
    for (const key of Object.keys(FLAG_DEFINITIONS)) {
      const env = 'ALLTERNIT_FLAG_' + key.toUpperCase().replace(/\./g, '_');
      if (process.env[env] !== undefined) {
        this.flags[key] = this.coerce(key, process.env[env]!);
      }
    }
  }

  private persist(): void {
    try {
      const dir = path.dirname(this.configPath);
      if (!fs.existsSync(dir)) fs.mkdirSync(dir, { recursive: true });
      // Only write non-default values to keep the file clean
      const overrides: Record<string, FlagValue> = {};
      for (const [key, value] of Object.entries(this.flags)) {
        if (FLAG_DEFINITIONS[key]?.default !== value) overrides[key] = value;
      }
      fs.writeFileSync(this.configPath, JSON.stringify(overrides, null, 2));
    } catch (err) {
      log.warn('[FeatureFlags] Failed to persist flags:', err);
    }
  }

  private startWatcher(): void {
    const dir = path.dirname(this.configPath);
    if (!fs.existsSync(dir)) return;
    try {
      this.watcher = fs.watch(this.configPath, { persistent: false }, () => {
        const prev = { ...this.flags };
        this.load();
        for (const key of Object.keys(FLAG_DEFINITIONS)) {
          if (prev[key] !== this.flags[key]) {
            log.info(`[FeatureFlags] Hot-reload: ${key} → ${this.flags[key]}`);
            this.emit(key, this.flags[key]);
          }
        }
      });
    } catch {
      // File may not exist yet; watcher starts when set() creates it
    }
  }

  private coerce(key: string, raw: string): FlagValue {
    const type = FLAG_DEFINITIONS[key]?.type ?? 'boolean';
    if (type === 'boolean') return raw === 'true' || raw === '1';
    if (type === 'number') return Number(raw);
    return raw;
  }

  private emit(key: string, value: FlagValue): void {
    for (const handler of this.handlers) {
      try { handler(key, value); } catch { /* never crash on a flag change */ }
    }
  }
}

export const featureFlagManager = new FeatureFlagManager();
