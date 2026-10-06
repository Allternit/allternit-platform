#!/usr/bin/env npx tsx
/**
 * Platform as Course Generator
 *
 * Analyzes the allternit-platform codebase and generates
 * a curriculum map for "How A://Labs Works" course.
 *
 * Usage:
 *   npx tsx scripts/platform-as-course.ts
 */

import * as fs from 'fs/promises';
import { existsSync } from 'node:fs';
import * as path from 'path';

const PLATFORM_DIR =
  process.env.ALLTERNIT_AI_PATH ||
  (existsSync('../allternit-ai/package.json') ? '../allternit-ai' : '');
if (!PLATFORM_DIR) {
  throw new Error('Workspace UI not found. Set ALLTERNIT_AI_PATH or clone Allternit/allternit-ai as a sibling.');
}

interface PlatformTopic {
  name: string;
  files: string[];
  concepts: string[];
  complexity: 'beginner' | 'intermediate' | 'advanced';
}

async function scanPlatform(): Promise<PlatformTopic[]> {
  const topics: PlatformTopic[] = [];

  // Define topic patterns
  const topicPatterns: Array<{ name: string; patterns: string[]; concepts: string[] }> = [
    {
      name: 'Next.js App Router & Layouts',
      patterns: ['src/app/**/layout.tsx', 'src/app/**/page.tsx', 'src/app/layout.tsx'],
      concepts: ['app router', 'layouts', 'nested routes', 'loading states'],
    },
    {
      name: 'Database Layer: SQLite + PostgreSQL',
      patterns: ['src/lib/db*', 'src/lib/prisma*', 'prisma/schema.prisma'],
      concepts: ['dual database', 'sqlite', 'postgresql', 'prisma', 'migrations'],
    },
    {
      name: 'Authentication & Authorization',
      patterns: ['src/lib/auth*', 'src/middleware*', 'src/app/api/auth/**'],
      concepts: ['auth', 'middleware', 'sessions', 'jwt', 'permissions'],
    },
    {
      name: 'API Routes & Middleware',
      patterns: ['src/app/api/**/route.ts', 'src/middleware.ts'],
      concepts: ['api routes', 'middleware', 'request handling', 'validation'],
    },
    {
      name: 'Canvas LMS Integration',
      patterns: ['src/lib/canvas*', 'src/app/**/canvas*'],
      concepts: ['canvas api', 'lms', 'course sync', 'oauth'],
    },
    {
      name: 'UI Components & Design System',
      patterns: ['src/components/**/*.tsx', 'src/components/ui/**'],
      concepts: ['react components', 'tailwind', 'design tokens', 'accessibility'],
    },
    {
      name: 'State Management & Caching',
      patterns: ['src/lib/state*', 'src/lib/cache*', 'src/hooks/**'],
      concepts: ['state', 'caching', 'react hooks', 'swr', 'zustand'],
    },
    {
      name: 'Build & Deployment Pipeline',
      patterns: ['next.config.*', 'Dockerfile', 'package.json', '.github/workflows/**'],
      concepts: ['vite build', 'docker', 'ci/cd', 'deployment', 'cloudflare pages'],
    },
  ];

  for (const topic of topicPatterns) {
    const matchedFiles: string[] = [];
    for (const pattern of topic.patterns) {
      const files = await globFiles(path.join(PLATFORM_DIR, pattern));
      matchedFiles.push(...files);
    }

    if (matchedFiles.length > 0) {
      topics.push({
        name: topic.name,
        files: [...new Set(matchedFiles)],
        concepts: topic.concepts,
        complexity: matchedFiles.length > 5 ? 'advanced' : matchedFiles.length > 2 ? 'intermediate' : 'beginner',
      });
    }
  }

  return topics;
}

async function globFiles(pattern: string): Promise<string[]> {
  const files: string[] = [];
  const dir = path.dirname(pattern);
  const base = path.basename(pattern);

  // Simple glob: handle ** and *
  if (base.includes('**')) {
    // Recursive search
    const ext = base.replace('**/', '').replace('*', '');
    await recursiveSearch(dir, ext, files);
  } else if (base.includes('*')) {
    // Single level wildcard
    const prefix = base.split('*')[0];
    const suffix = base.split('*')[1] || '';
    try {
      const entries = await fs.readdir(dir, { withFileTypes: true });
      for (const entry of entries) {
        if (entry.name.startsWith(prefix) && entry.name.endsWith(suffix)) {
          files.push(path.join(dir, entry.name));
        }
      }
    } catch { /* ignore */ }
  } else {
    // Exact file
    try {
      await fs.access(pattern);
      files.push(pattern);
    } catch { /* ignore */ }
  }

  return files;
}

async function recursiveSearch(dir: string, ext: string, files: string[]) {
  try {
    const entries = await fs.readdir(dir, { withFileTypes: true });
    for (const entry of entries) {
      const fullPath = path.join(dir, entry.name);
      if (entry.isDirectory() && !entry.name.startsWith('.') && entry.name !== 'node_modules') {
        await recursiveSearch(fullPath, ext, files);
      } else if (entry.name.endsWith(ext)) {
        files.push(fullPath);
      }
    }
  } catch { /* ignore */ }
}

function generateCourseOutline(topics: PlatformTopic[]) {
  const modules = topics.map((t, i) => ({
    position: i + 1,
    title: `Module ${i + 1}: ${t.name}`,
    complexity: t.complexity,
    concepts: t.concepts,
    keyFiles: t.files.slice(0, 5), // Top 5 files
    learningObjectives: t.concepts.map(c => `Understand how ${c} works in the A://Labs platform`),
  }));

  return {
    courseCode: 'ALABS-PLATFORM',
    title: 'How A://Labs Works',
    subtitle: 'A deep dive into the Allternit platform architecture',
    tier: 'ADV',
    totalModules: modules.length,
    estimatedHours: modules.length * 3,
    modules,
  };
}

async function main() {
  console.log('🔍 Scanning platform codebase...\n');
  const topics = await scanPlatform();

  console.log(`Found ${topics.length} topics:\n`);
  for (const t of topics) {
    console.log(`📁 ${t.name} (${t.complexity})`);
    console.log(`   Files: ${t.files.length}`);
    console.log(`   Concepts: ${t.concepts.join(', ')}`);
    console.log('');
  }

  const course = generateCourseOutline(topics);

  const outputDir = 'alabs-generated-courses/analysis';
  await fs.mkdir(outputDir, { recursive: true });

  const outputPath = path.join(outputDir, 'platform-course-outline.json');
  await fs.writeFile(outputPath, JSON.stringify(course, null, 2), 'utf-8');
  console.log(`💾 Course outline saved to ${outputPath}`);
  console.log(`\n📚 ${course.title}: ${course.totalModules} modules, ~${course.estimatedHours} hours`);
}

main().catch(err => {
  console.error(err);
  process.exit(1);
});
