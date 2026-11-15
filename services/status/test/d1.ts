// Test-only D1 stand-in over node:sqlite (real SQLite, real schema from migrations/).
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { DatabaseSync } from "node:sqlite";
import type { Db, Stmt } from "../src/store";

class SqliteStmt implements Stmt {
  constructor(
    private db: DatabaseSync,
    private sql: string,
    private values: unknown[] = [],
  ) {}
  bind(...values: unknown[]): Stmt {
    return new SqliteStmt(this.db, this.sql, values);
  }
  private args() {
    return this.values.map((v) => (typeof v === "boolean" ? (v ? 1 : 0) : v === undefined ? null : v)) as never[];
  }
  exec(): { results: Record<string, unknown>[] } {
    const st = this.db.prepare(this.sql);
    if (/^\s*(SELECT|WITH)/i.test(this.sql)) return { results: st.all(...this.args()) as Record<string, unknown>[] };
    st.run(...this.args());
    return { results: [] };
  }
  async all<T>() {
    return this.exec() as unknown as { results: T[] };
  }
  async first<T>() {
    return ((this.exec().results[0] as T) ?? null) as T | null;
  }
  async run() {
    return this.exec();
  }
}

export class TestD1 implements Db {
  readonly raw = new DatabaseSync(":memory:");
  constructor() {
    const sql = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "../migrations/0001_init.sql"), "utf8");
    this.raw.exec(sql);
  }
  prepare(sql: string): Stmt {
    return new SqliteStmt(this.raw, sql);
  }
  async batch<T>(stmts: Stmt[]): Promise<{ results: T[] }[]> {
    this.raw.exec("BEGIN");
    try {
      const out = stmts.map((s) => (s as SqliteStmt).exec());
      this.raw.exec("COMMIT");
      return out as unknown as { results: T[] }[];
    } catch (e) {
      this.raw.exec("ROLLBACK");
      throw e;
    }
  }
  rows<T = Record<string, unknown>>(sql: string, ...args: unknown[]): T[] {
    return this.raw.prepare(sql).all(...(args as never[])) as T[];
  }
}
