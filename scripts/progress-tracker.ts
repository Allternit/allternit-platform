#!/usr/bin/env npx tsx
/**
 * Student Progress Tracker for A://Labs
 *
 * Polls Canvas for module/item completion and updates
 * the local SQLite certifications database.
 *
 * Usage:
 *   npx tsx scripts/progress-tracker.ts --user-id <id>
 */

import Database from 'better-sqlite3';
import { existsSync } from 'node:fs';

import { getCanvasToken } from './canvas-token.ts';
const CANVAS_TOKEN = getCanvasToken();
const BASE_URL = 'https://canvas.instructure.com/api/v1';

const COURSES = [
  { id: '14593493', code: 'ALABS-CORE-COPILOT' },
  { id: '14593495', code: 'ALABS-CORE-PROMPTS' },
  { id: '14593499', code: 'ALABS-OPS-N8N' },
  { id: '14593501', code: 'ALABS-OPS-VISION' },
  { id: '14593503', code: 'ALABS-OPS-RAG' },
  { id: '14593505', code: 'ALABS-AGENTS-ML' },
  { id: '14593507', code: 'ALABS-AGENTS-AGENTS' },
  { id: '14612851', code: 'ALABS-ADV-PLUGINSDK' },
  { id: '14612861', code: 'ALABS-ADV-WORKFLOW' },
  { id: '14612869', code: 'ALABS-ADV-ADAPTERS' },
];

interface ModuleProgress {
  moduleId: number;
  moduleName: string;
  position: number;
  items: Array<{
    itemId: number;
    title: string;
    type: string;
    completed: boolean;
  }>;
}

async function canvasApi(method: string, pathStr: string) {
  const url = `${BASE_URL}${pathStr}`;
  const res = await fetch(url, {
    method,
    headers: { Authorization: `Bearer ${CANVAS_TOKEN}` },
  });
  if (!res.ok) throw new Error(`Canvas API ${method} ${pathStr} failed: ${res.status}`);
  return res.json();
}

async function getModuleProgress(courseId: string, userId: string): Promise<ModuleProgress[]> {
  const modules = await canvasApi('GET', `/courses/${courseId}/modules?student_id=${userId}&per_page=100&include[]=items`) as any[];
  return modules.map(m => ({
    moduleId: m.id,
    moduleName: m.name,
    position: m.position,
    items: (m.items || []).map((item: any) => ({
      itemId: item.id,
      title: item.title,
      type: item.type,
      completed: item.completion_requirement?.completed || false,
    })),
  }));
}

function getDb() {
  const ui =
    process.env.ALLTERNIT_AI_PATH ||
    (existsSync('../allternit-ai/package.json') ? '../allternit-ai' : '');
  if (!ui) {
    throw new Error('Workspace UI not found. Set ALLTERNIT_AI_PATH or clone Allternit/allternit-ai as a sibling.');
  }
  return new Database(`${ui}/data/sqlite.db`);
}

function updateProgress(db: Database, userId: string, courseCode: string, progress: ModuleProgress[]) {
  const totalItems = progress.reduce((sum, m) => sum + m.items.length, 0);
  const completedItems = progress.reduce((sum, m) => sum + m.items.filter(i => i.completed).length, 0);
  const percentComplete = totalItems > 0 ? Math.round((completedItems / totalItems) * 100) : 0;
  const allModulesComplete = progress.every(m => m.items.every(i => i.completed));

  const stmt = db.prepare(`
    INSERT INTO certifications (userId, courseCode, progress, completedAt, status, updatedAt)
    VALUES (?, ?, ?, ?, ?, ?)
    ON CONFLICT(userId, courseCode) DO UPDATE SET
      progress = excluded.progress,
      completedAt = CASE WHEN excluded.status = 'completed' THEN datetime('now') ELSE completedAt END,
      status = excluded.status,
      updatedAt = datetime('now')
  `);

  stmt.run(
    userId,
    courseCode,
    percentComplete,
    allModulesComplete ? new Date().toISOString() : null,
    allModulesComplete ? 'completed' : 'in_progress',
    new Date().toISOString()
  );

  return { percentComplete, allModulesComplete };
}

async function main() {
  const args = process.argv.slice(2);
  const userIdx = args.indexOf('--user-id');

  if (userIdx === -1) {
    console.error('Usage: npx tsx progress-tracker.ts --user-id <canvas_user_id>');
    process.exit(1);
  }

  const userId = args[userIdx + 1];
  const db = getDb();

  console.log(`👤 Tracking progress for user ${userId}...\n`);

  for (const course of COURSES) {
    try {
      const progress = await getModuleProgress(course.id, userId);
      const { percentComplete, allModulesComplete } = updateProgress(db, userId, course.code, progress);

      const status = allModulesComplete ? '✅ COMPLETE' : `${percentComplete}%`;
      const moduleSummary = progress.map(m => {
        const done = m.items.filter(i => i.completed).length;
        const total = m.items.length;
        return `    M${m.position}: ${done}/${total}`;
      }).join('\n');

      console.log(`${course.code}: ${status}`);
      console.log(moduleSummary);
      console.log('');
    } catch (err: any) {
      console.log(`${course.code}: ⚠️  ${err.message}`);
    }
  }

  db.close();
  console.log('💾 Progress saved to SQLite');
}

main().catch(err => {
  console.error(err);
  process.exit(1);
});
