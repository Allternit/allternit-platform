import { defineConfig } from 'vitest/config';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, '..');

export default defineConfig({
  resolve: {
    alias: {},
  },
  test: {
    globals: true,
    environment: 'node',
    setupFiles: ['./setup-localstorage.ts'],
    include: [
      'integration/**/*.test.ts',
      'e2e/**/*.test.ts',
    ],
    exclude: [
      'node_modules',
      'dist',
      // Workspace UI tests live in Allternit/allternit-ai now.
      'integration/cowork-team.test.ts',
      'integration/intelli-schedule.test.ts',
    ],
    testTimeout: 30000,
    hookTimeout: 10000,
    coverage: {
      provider: 'v8',
      reporter: ['text', 'html', 'json'],
      exclude: [
        'node_modules/',
        'dist/',
        'tests/',
        '**/*.config.ts',
      ],
    },
  },
});
