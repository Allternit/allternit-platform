// SQLite through Node's built-in node:sqlite, with the small better-sqlite3
// surface the gateway uses (prepare/run/get/all, exec, transaction, pragma,
// close). No native module: Allternit Desktop runs the gateway on Electron's
// own Node (ELECTRON_RUN_AS_NODE), where a prebuilt better-sqlite3 would need
// a separate build per Electron ABI and platform. Same engine as the
// open-connector sidecar.
//
// Differences from better-sqlite3 that this wrapper hides:
// - BLOB columns come back as Uint8Array; rows convert them to Buffer so
//   callers keep Buffer methods (indexOf, toString("hex")).
// - The busy timeout defaults to 0; better-sqlite3 waits 5 s, so this does too.
// - close() on a closed database throws; better-sqlite3 ignores it, so this does too.
import { existsSync } from "node:fs";
import { createRequire } from "node:module";
import type { DatabaseSync as DatabaseSyncT, StatementSync, SQLInputValue } from "node:sqlite";

// Loaded through require: vite 5 (vitest 1) doesn't know node:sqlite and
// strips the prefix, so a static import fails under the test runner.
const { DatabaseSync } = createRequire(import.meta.url)("node:sqlite") as typeof import("node:sqlite");

export interface RunResult {
  /** A number: the database never turns on readBigInts. */
  changes: number;
  lastInsertRowid: number | bigint;
}

export interface OpenOptions {
  readonly?: boolean;
  /** Fail instead of creating the file (better-sqlite3's fileMustExist). Read-only opens always need the file. */
  fileMustExist?: boolean;
  /** Busy timeout in ms (default 5000). */
  timeout?: number;
}

// Like better-sqlite3, callers pass values of any type; node:sqlite rejects ones it can't bind.
type Param = unknown;

function toBuffers<T>(row: T): T {
  if (!row || typeof row !== "object") return row;
  const r = row as Record<string, unknown>;
  for (const k of Object.keys(r)) {
    const v = r[k];
    if (v instanceof Uint8Array && !Buffer.isBuffer(v)) r[k] = Buffer.from(v.buffer, v.byteOffset, v.byteLength);
  }
  return row;
}

export class Statement {
  constructor(private readonly stmt: StatementSync) {}

  run(...params: Param[]): RunResult {
    const r = this.stmt.run(...(params as SQLInputValue[]));
    return { changes: Number(r.changes), lastInsertRowid: r.lastInsertRowid };
  }

  get(...params: Param[]): unknown {
    return toBuffers(this.stmt.get(...(params as SQLInputValue[])));
  }

  all(...params: Param[]): unknown[] {
    return this.stmt.all(...(params as SQLInputValue[])).map(toBuffers);
  }
}

export class Database {
  private readonly db: DatabaseSyncT;
  private depth = 0;

  constructor(path: string, options: OpenOptions = {}) {
    if (options.fileMustExist && !options.readonly && path !== ":memory:") {
      // node:sqlite has no "must exist" flag for read-write opens; check first.
      if (!existsSync(path)) throw new Error(`unable to open database file: ${path}`);
    }
    this.db = new DatabaseSync(path, { readOnly: options.readonly ?? false, timeout: options.timeout ?? 5000 });
  }

  prepare(sql: string): Statement {
    return new Statement(this.db.prepare(sql));
  }

  exec(sql: string): this {
    this.db.exec(sql);
    return this;
  }

  /** `PRAGMA <source>`; returns the result rows like better-sqlite3 (without its `simple` option). */
  pragma(source: string): unknown[] {
    return this.db.prepare(`PRAGMA ${source}`).all();
  }

  /**
   * better-sqlite3's transaction(): returns a function that runs `fn` inside
   * BEGIN/COMMIT and rolls back on throw. Nested calls use savepoints.
   */
  transaction<A extends unknown[], R>(fn: (...args: A) => R): (...args: A) => R {
    return (...args: A): R => {
      const sp = `sp${this.depth}`;
      this.db.exec(this.depth === 0 ? "BEGIN" : `SAVEPOINT ${sp}`);
      this.depth++;
      try {
        const out = fn(...args);
        this.depth--;
        this.db.exec(this.depth === 0 ? "COMMIT" : `RELEASE ${sp}`);
        return out;
      } catch (e) {
        this.depth--;
        this.db.exec(this.depth === 0 ? "ROLLBACK" : `ROLLBACK TO ${sp}; RELEASE ${sp}`);
        throw e;
      }
    };
  }

  /** False once closed. */
  get open(): boolean {
    return this.db.isOpen;
  }

  /** Idempotent, like better-sqlite3 (node:sqlite throws on a second close). */
  close(): void {
    if (this.db.isOpen) this.db.close();
  }
}

export default Database;
