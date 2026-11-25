/**
 * Persisted Atom State — structured, versioned, debounced state persistence.
 *
 * Replaces raw electron-store access with typed state slices, schema
 * migrations, and a subscribe API so the renderer can react to changes.
 *
 * Uses electron-store as the durable backend (same file, extended API).
 */

import Store from 'electron-store';
import log from 'electron-log';
import { z } from 'zod';

// ── Schema version — bump when adding/removing/renaming fields ───────────────
const SCHEMA_VERSION = 2;

// ── Zod Schemas ──────────────────────────────────────────────────────────────

export const WindowStateSchema = z.object({
  width: z.number().default(1400),
  height: z.number().default(900),
  x: z.number().optional(),
  y: z.number().optional(),
  maximized: z.boolean().default(false),
});

export const BackendStateSchema = z.object({
  mode: z.enum(['bundled', 'remote', 'development']).default('bundled'),
  remoteUrl: z.string().optional(),
  lastLocalVersion: z.string().optional(),
});

export const SessionStateSchema = z.object({
  lastSessionId: z.string().optional(),
  lastProjectPath: z.string().optional(),
  recentSessionIds: z.array(z.string()).default([]),
});

export const AppPrefsSchema = z.object({
  theme: z.enum(['light', 'dark', 'system']).default('system'),
  locale: z.string().optional(),
  menuBarMode: z.boolean().default(false),
  startupOnLogin: z.boolean().default(false),
  keepRunningInTray: z.boolean().default(false),
  viewMode: z.enum(['verbose', 'normal', 'summary']).default('normal'),
});

export const SidecarStateSchema = z.object({
  apiUrl: z.string(),
  port: z.number(),
}).nullable();

export const StateSchema = z.object({
  _version: z.number().default(SCHEMA_VERSION),
  window: WindowStateSchema,
  backend: BackendStateSchema,
  session: SessionStateSchema,
  prefs: AppPrefsSchema,
  onboardingComplete: z.boolean().default(false),
  sidecar: SidecarStateSchema,
});

export type WindowState = z.infer<typeof WindowStateSchema>;
export type BackendState = z.infer<typeof BackendStateSchema>;
export type SessionState = z.infer<typeof SessionStateSchema>;
export type AppPrefs = z.infer<typeof AppPrefsSchema>;
export type SidecarState = z.infer<typeof SidecarStateSchema>;
export type PersistedStateSchema = z.infer<typeof StateSchema>;

// ── Defaults ─────────────────────────────────────────────────────────────────

const DEFAULTS: PersistedStateSchema = {
  _version: SCHEMA_VERSION,
  window: { width: 1400, height: 900, maximized: false },
  backend: { mode: 'bundled' },
  session: { recentSessionIds: [] },
  prefs: {
    theme: 'system',
    menuBarMode: false,
    startupOnLogin: false,
    keepRunningInTray: false,
    viewMode: 'normal',
  },
  onboardingComplete: false,
  sidecar: null,
};

// ── Migrations ───────────────────────────────────────────────────────────────
// Add entries here when SCHEMA_VERSION bumps.

type Migration = (raw: Record<string, unknown>) => void;

const MIGRATIONS: Record<number, Migration> = {
  2: (raw) => {
    if (raw.sidecar && typeof raw.sidecar === 'object') {
      delete (raw.sidecar as Record<string, unknown>).password;
    }
  },
};

// ── Manager ──────────────────────────────────────────────────────────────────

type SliceKey = keyof Omit<PersistedStateSchema, '_version'>;
type SliceValue<K extends SliceKey> = PersistedStateSchema[K];
type ChangeHandler<K extends SliceKey> = (value: SliceValue<K>) => void;

const SCHEMAS: Record<SliceKey, z.ZodTypeAny> = {
  window: WindowStateSchema,
  backend: BackendStateSchema,
  session: SessionStateSchema,
  prefs: AppPrefsSchema,
  onboardingComplete: z.boolean(),
  sidecar: SidecarStateSchema,
};

class PersistedStateManager {
  private store: Store<PersistedStateSchema>;
  private handlers = new Map<string, Set<ChangeHandler<SliceKey>>>();
  private flushTimer: ReturnType<typeof setTimeout> | null = null;
  private pending: Partial<PersistedStateSchema> = {};

  constructor() {
    this.store = new Store<PersistedStateSchema>({
      name: 'allternit-state',
      defaults: DEFAULTS,
    });
    this.migrate();
  }

  get<K extends SliceKey>(key: K): SliceValue<K> {
    return this.store.get(key) as SliceValue<K>;
  }

  set<K extends SliceKey>(key: K, value: SliceValue<K>): void {
    const schema = SCHEMAS[key];
    if (schema) {
      const parsed = schema.safeParse(value);
      if (!parsed.success) {
        log.error(`[PersistedState] Validation failed for key "${key}":`, parsed.error.format());
        throw new Error(`Validation failed for key "${key}": ${parsed.error.message}`);
      }
      value = parsed.data as SliceValue<K>;
    }
    this.pending[key] = value as never;
    this.scheduleFLush();
    this.emit(key, value);
  }

  /** Deep-merge a partial update into a slice. */
  patch<K extends SliceKey>(key: K, partial: Partial<SliceValue<K>>): void {
    const current = this.get(key);
    const next = { ...(current as object), ...(partial as object) } as SliceValue<K>;
    this.set(key, next);
  }

  subscribe<K extends SliceKey>(key: K, handler: ChangeHandler<K>): () => void {
    if (!this.handlers.has(key)) this.handlers.set(key, new Set());
    this.handlers.get(key)!.add(handler as ChangeHandler<SliceKey>);
    return () => this.handlers.get(key)?.delete(handler as ChangeHandler<SliceKey>);
  }

  /** Force-flush pending writes immediately (call on app quit). */
  flush(): void {
    if (this.flushTimer) {
      clearTimeout(this.flushTimer);
      this.flushTimer = null;
    }
    this.writePending();
  }

  // ── Private ────────────────────────────────────────────────────────────────

  private scheduleFLush(): void {
    if (this.flushTimer) clearTimeout(this.flushTimer);
    this.flushTimer = setTimeout(() => {
      this.flushTimer = null;
      this.writePending();
    }, 500);
  }

  private writePending(): void {
    if (Object.keys(this.pending).length === 0) return;
    for (const [key, value] of Object.entries(this.pending)) {
      this.store.set(key as SliceKey, value as never);
    }
    log.debug('[PersistedState] Flushed:', Object.keys(this.pending));
    this.pending = {};
  }

  private migrate(): void {
    const stored = (this.store.get('_version') as number | undefined) ?? 0;
    if (stored >= SCHEMA_VERSION) return;
    log.info(`[PersistedState] Migrating v${stored} → v${SCHEMA_VERSION}`);
    const raw = this.store.store as unknown as Record<string, unknown>;
    for (let v = stored + 1; v <= SCHEMA_VERSION; v++) {
      const migration = MIGRATIONS[v];
      if (migration) {
        try { migration(raw); } catch (err) {
          log.error(`[PersistedState] Migration v${v} failed:`, err);
        }
      }
    }
    raw['_version'] = SCHEMA_VERSION;
    this.store.store = raw as unknown as PersistedStateSchema;
    log.info('[PersistedState] Migration complete');
  }

  private emit<K extends SliceKey>(key: K, value: SliceValue<K>): void {
    const set = this.handlers.get(key);
    if (!set) return;
    for (const handler of set) {
      try { handler(value as never); } catch { /* never crash on state change */ }
    }
  }
}

export const persistedState = new PersistedStateManager();
