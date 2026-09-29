/**
 * Migration-chain tests for the startup schema migrations.
 *
 * Production applies these via the bundled GIZZI_MIGRATIONS injection
 * (script/build-production.js); dev/test applies the same folders from
 * migration/ (src/runtime/session/storage/db.ts, "dev" mode). Neither
 * test/preload.ts nor ci-smoke-test.sh performed any migration/schema check,
 * so a broken or mis-ordered migration.sql would only surface at runtime.
 *
 * These tests run the real journal loader + drizzle migrator used by
 * Database.Client against fresh temp DBs and assert the final schema.
 */

import { describe, expect, test } from "bun:test"
import { Database as BunDatabase } from "bun:sqlite"
import { drizzle } from "drizzle-orm/bun-sqlite"
import { readdirSync } from "fs"
import path from "path"
import { Database } from "@/runtime/session/storage/db"
import * as schema from "@/runtime/session/storage/schema"
import { tmpdir } from "../fixture/fixture"

const MIGRATION_DIR = path.join(import.meta.dir, "../../migration")

function openDb(file: string) {
  const sqlite = new BunDatabase(file, { create: true })
  sqlite.run("PRAGMA journal_mode = WAL")
  sqlite.run("PRAGMA foreign_keys = ON")
  return { sqlite, db: drizzle({ client: sqlite, schema }) }
}

function tables(sqlite: BunDatabase): string[] {
  return sqlite
    .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
    .all()
    .map((r) => (r as { name: string }).name)
}

function columns(sqlite: BunDatabase, table: string): string[] {
  return sqlite
    .prepare(`PRAGMA table_info("${table}")`)
    .all()
    .map((r) => (r as { name: string }).name)
}

function appliedCount(sqlite: BunDatabase): number {
  const row = sqlite.prepare('SELECT COUNT(*) AS n FROM "__drizzle_migrations"').get() as
    | { n: number }
    | undefined
  return Number(row?.n ?? 0)
}

describe("migration journal layout", () => {
  const names = readdirSync(MIGRATION_DIR, { withFileTypes: true })
    .filter((e) => e.isDirectory())
    .map((e) => e.name)
    .sort()

  test("every migration folder has a migration.sql with a parseable 14-digit timestamp prefix", () => {
    expect(names.length).toBeGreaterThanOrEqual(16)
    for (const name of names) {
      expect(name, `migration folder ${name}`).toMatch(/^\d{14}_/)
    }
    const journal = Database.loadMigrations(MIGRATION_DIR)
    expect(journal.length).toBe(names.length)
    for (const entry of journal) {
      expect(entry.timestamp, "timestamp parsed from folder name").toBeGreaterThan(0)
      expect(entry.hash).toMatch(/^[0-9a-f]{64}$/)
    }
  })

  test("lexicographic folder order equals timestamp order (a mis-named migration would apply out of order)", () => {
    const stamps = names.map((name) => {
      const m = /^(\d{4})(\d{2})(\d{2})(\d{2})(\d{2})(\d{2})/.exec(name)
      expect(m, `14-digit timestamp prefix in ${name}`).not.toBeNull()
      return Date.UTC(
        Number(m![1]),
        Number(m![2]) - 1,
        Number(m![3]),
        Number(m![4]),
        Number(m![5]),
        Number(m![6]),
      )
    })
    const sorted = [...stamps].sort((a, b) => a - b)
    expect(stamps).toEqual(sorted)
    for (let i = 1; i < stamps.length; i++) {
      expect(stamps[i], `strictly increasing timestamp at ${names[i]}`).toBeGreaterThan(stamps[i - 1]!)
    }
  })
})

describe("migration chain on a fresh DB (what startup runs)", () => {
  test("applies every migration and lands on the current schema", async () => {
    await using tmp = await tmpdir()
    const { sqlite, db } = openDb(path.join(tmp.path, "fresh.db"))

    const journal = Database.loadMigrations(MIGRATION_DIR)
    Database.applyMigrations(db, journal)

    expect(appliedCount(sqlite)).toBe(journal.length)

    // Real tables from across the chain
    const t = tables(sqlite)
    for (const table of [
      "project", // 20260127222353
      "control_account", // 20260213144116
      "cowork_run", // 20260414094100
      "brain_memory_chunk", // 20260414104500
      "routine", // 20260718120000 automation tables
      "background_task", // 20260718120300
      "session_trace", // 20260718120400
      "runtime", // 20260818000000
      "session_source_event", // 20260906180000
    ]) {
      expect(t, `table ${table}`).toContain(table)
    }

    // Real columns added by later ALTER migrations
    expect(columns(sqlite, "project")).toContain("commands") // 20260211171708
    const sessionCols = columns(sqlite, "session")
    for (const col of ["surface", "harness", "agent_id", "pinned", "permission_mode", "default_model", "source_harness", "source_session_id", "source_snapshot_hash", "source_export", "paused", "limit_state"]) {
      expect(sessionCols, `session.${col}`).toContain(col)
    }
    expect(columns(sqlite, "goal")).toContain("budget") // 20260718120100

    sqlite.close()
  })
})

describe("migration chain on an old DB (upgrade regression)", () => {
  test("a DB at an early migration upgrades to the current schema", async () => {
    await using tmp = await tmpdir()
    const { sqlite, db } = openDb(path.join(tmp.path, "old.db"))
    const journal = Database.loadMigrations(MIGRATION_DIR)

    // Simulate a DB created by the first 3 migrations only
    const early = journal.slice(0, 3)
    Database.applyMigrations(db, early)
    expect(appliedCount(sqlite)).toBe(3)
    expect(columns(sqlite, "session"), "pre-upgrade session has no agent_id yet").not.toContain("agent_id")

    // Startup against the old DB applies only the missing tail
    Database.applyMigrations(db, journal)
    expect(appliedCount(sqlite)).toBe(journal.length)

    const sessionCols = columns(sqlite, "session")
    for (const col of ["agent_id", "pinned", "permission_mode", "default_model", "source_harness", "source_snapshot_hash"]) {
      expect(sessionCols, `session.${col} after upgrade`).toContain(col)
    }
    expect(columns(sqlite, "goal")).toContain("budget")
    expect(tables(sqlite)).toContain("background_task")
    expect(tables(sqlite)).toContain("session_source_event")

    sqlite.close()
  })
})
