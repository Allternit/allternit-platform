// §1 store — better-sqlite3 (WAL) + file-ordered, idempotent migration runner.
import Database from "better-sqlite3";
import { mkdirSync, readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export type Db = Database.Database;

const DEFAULT_MIGRATIONS_DIR = fileURLToPath(
  new URL("./migrations/", import.meta.url)
);

export interface OpenDatabaseOptions {
  migrationsDir?: string;
}

export function openDatabase(
  path: string,
  options: OpenDatabaseOptions = {}
): Db {
  if (path !== ":memory:") {
    mkdirSync(dirname(path), { recursive: true });
  }
  const db = new Database(path);
  db.pragma("journal_mode = WAL");
  db.pragma("foreign_keys = ON");
  runMigrations(db, options.migrationsDir ?? DEFAULT_MIGRATIONS_DIR);
  return db;
}

interface MigrationRow {
  version: number;
}

// Applies *.sql files in filename order; the `migrations` ledger makes
// re-runs a no-op, so booting an existing database applies nothing twice.
export function runMigrations(db: Db, migrationsDir: string): void {
  db.exec(
    "CREATE TABLE IF NOT EXISTS migrations (" +
      "version INTEGER PRIMARY KEY, " +
      "applied_at TEXT NOT NULL" +
      ")"
  );
  const applied = new Set(
    (db.prepare("SELECT version FROM migrations").all() as MigrationRow[]).map(
      (r) => r.version
    )
  );
  const files = readdirSync(migrationsDir)
    .filter((f) => /^\d+.*\.sql$/.test(f))
    .sort();
  const insertVersion = db.prepare(
    "INSERT INTO migrations (version, applied_at) VALUES (?, ?)"
  );
  for (const file of files) {
    const version = parseInt(file, 10);
    if (applied.has(version)) continue;
    const sql = readFileSync(join(migrationsDir, file), "utf8");
    db.transaction(() => {
      db.exec(sql);
      insertVersion.run(version, new Date().toISOString());
    })();
  }
}
