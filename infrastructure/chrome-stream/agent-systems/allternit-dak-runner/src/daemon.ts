#!/usr/bin/env node
/**
 * DAK Runner Daemon
 *
 * Long-running sidecar that continuously discovers and executes
 * work items from the Rails control plane.
 *
 * Implements LAW-AUT-001: No-Stop Execution Rule
 * - Never idles while READY nodes exist
 * - Deterministic ordering (priority, then nodeId)
 * - Budget-bounded execution
 *
 * Usage:
 *   dak-daemon                     # Start with defaults
 *   dak-daemon --max-concurrent 5  # Allow 5 parallel nodes
 *   dak-daemon --budget-max 80     # Stop at 80% budget usage
 *
 * Environment:
 *   Allternit_RAILS_CLI        Rails CLI path (default: allternit)
 *   Allternit_RAILS_HTTP_URL   Rails HTTP API URL
 *   Allternit_RAILS_API_KEY    Rails API key
 *   Allternit_PROJECT_PATH     Project root (default: cwd)
 *   Allternit_OUTPUT_DIR       Output directory (default: .allternit/runner)
 *   Allternit_MAX_CONCURRENT   Max parallel nodes (default: 3)
 *   Allternit_BUDGET_MAX       Max budget percentage (default: 90)
 */

import { RailsHttpAdapter, createRailsHttpAdapter } from './adapters/rails_http';
import { createRailsUnifiedAdapter, RailsUnifiedAdapter } from './adapters/rails_unified';
import { RalphLoop, createRalphLoop } from './loop/ralph';
import { RalphNoStopScheduler, createRalphNoStopScheduler, SchedulerConfig } from './loop/no-stop-scheduler';
import { ContextPackBuilder, createContextPackBuilder } from './context/builder';
import { WorkerManager, createWorkerManager } from './workers/manager';
import { PlanManager, createPlanManager } from './plan/manager';

interface DaemonConfig {
  maxConcurrent: number;
  budgetMax: number;
  budgetCheckIntervalMs: number;
  railsCliPath: string;
  railsHttpUrl?: string;
  railsApiKey?: string;
  projectPath: string;
  outputDir: string;
}

function parseDaemonArgs(): DaemonConfig {
  const args = process.argv.slice(2);
  const config: DaemonConfig = {
    maxConcurrent: parseInt(process.env.Allternit_MAX_CONCURRENT || '3', 10),
    budgetMax: parseInt(process.env.Allternit_BUDGET_MAX || '90', 10),
    budgetCheckIntervalMs: 5000,
    railsCliPath: process.env.Allternit_RAILS_CLI || 'allternit',
    railsHttpUrl: process.env.Allternit_RAILS_HTTP_URL,
    railsApiKey: process.env.Allternit_RAILS_API_KEY,
    projectPath: process.env.Allternit_PROJECT_PATH || process.cwd(),
    outputDir: process.env.Allternit_OUTPUT_DIR || '.allternit/runner',
  };

  for (let i = 0; i < args.length; i++) {
    switch (args[i]) {
      case '--max-concurrent':
        config.maxConcurrent = parseInt(args[++i], 10);
        break;
      case '--budget-max':
        config.budgetMax = parseInt(args[++i], 10);
        break;
      case '--project':
        config.projectPath = args[++i];
        break;
      case '--output':
        config.outputDir = args[++i];
        break;
      case '--help':
      case '-h':
        printUsage();
        process.exit(0);
    }
  }

  return config;
}

function printUsage(): void {
  process.stdout.write(`
DAK Runner Daemon - Continuous agent execution

Usage: dak-daemon [options]

Options:
  --max-concurrent <n>   Max parallel nodes (default: 3)
  --budget-max <n>       Max budget percentage (default: 90)
  --project <path>       Project root path
  --output <path>        Output directory
  -h, --help             Show this help

Environment:
  Allternit_RAILS_CLI          Rails CLI path
  Allternit_RAILS_HTTP_URL     Rails HTTP API URL
  Allternit_RAILS_API_KEY      Rails API key
  Allternit_PROJECT_PATH       Project root
  Allternit_OUTPUT_DIR         Output directory
  Allternit_MAX_CONCURRENT     Max parallel nodes
  Allternit_BUDGET_MAX         Max budget percentage
`);
}

function log(level: string, msg: string, data?: Record<string, unknown>): void {
  const entry = {
    ts: new Date().toISOString(),
    level,
    msg,
    ...data,
  };
  process.stdout.write(JSON.stringify(entry) + '\n');
}

async function startDaemon(config: DaemonConfig): Promise<void> {
  log('info', 'DAK daemon starting', {
    maxConcurrent: config.maxConcurrent,
    budgetMax: config.budgetMax,
    projectPath: config.projectPath,
    mode: config.railsHttpUrl ? 'http' : 'cli',
  });

  // Build Rails adapter (HTTP preferred)
  let railsHttp: RailsHttpAdapter;
  if (config.railsHttpUrl) {
    railsHttp = createRailsHttpAdapter({
      baseURL: config.railsHttpUrl,
      apiKey: config.railsApiKey,
    });
  } else {
    // Use unified adapter's HTTP mode with a local fallback
    railsHttp = createRailsHttpAdapter({
      baseURL: 'http://localhost:3010/api/factory',
    });
  }

  // Build components
  const workerManager = createWorkerManager();
  const planManager = createPlanManager(`${config.outputDir}/plans`);
  const contextPackBuilder = createContextPackBuilder({ basePath: config.projectPath });
  const ralphLoop = createRalphLoop(workerManager, planManager, {
    maxFixCycles: 3,
    enableParallelValidation: false,
  });

  // Build scheduler
  const schedulerConfig: SchedulerConfig = {
    maxConcurrentNodes: config.maxConcurrent,
    budgetCheckIntervalMs: config.budgetCheckIntervalMs,
    maxBudgetPercentage: config.budgetMax,
  };

  const scheduler = createRalphNoStopScheduler(
    railsHttp,
    ralphLoop,
    contextPackBuilder,
    schedulerConfig,
  );

  // Wire events to structured logging
  scheduler.on('scheduler:started', () => log('info', 'Scheduler started'));
  scheduler.on('scheduler:stopped', () => log('info', 'Scheduler stopped'));
  scheduler.on('scheduler:paused', (data) => log('warn', 'Scheduler paused', data));
  scheduler.on('work:discovered', (data) => log('info', 'Work discovered', data));
  scheduler.on('work:discovery_error', (data) => log('error', 'Work discovery error', data));
  scheduler.on('node:started', (data) => log('info', 'Node started', data));
  scheduler.on('node:completed', (data) => log('info', 'Node completed', data));
  scheduler.on('node:failed', (data) => log('warn', 'Node failed', data));
  scheduler.on('node:blocked', (data) => log('warn', 'Node blocked', data));
  scheduler.on('node:error', (data) => log('error', 'Node error', data));
  scheduler.on('budget:updated', (data) => {
    if (data.usedPercentage > data.maxPercentage * 0.9) {
      log('warn', 'Budget nearing limit', data);
    }
  });

  // Worker health monitoring
  workerManager.startHealthMonitoring();
  workerManager.on('worker:unhealthy', (data) => {
    log('error', 'Worker unhealthy', data);
  });

  // Graceful shutdown
  let shuttingDown = false;
  const shutdown = async (signal: string) => {
    if (shuttingDown) return;
    shuttingDown = true;

    log('info', 'Shutdown requested', { signal });
    scheduler.stop();
    workerManager.stopHealthMonitoring();

    // Terminate unhealthy workers
    const terminated = await workerManager.terminateUnhealthyWorkers();
    if (terminated > 0) {
      log('info', 'Terminated unhealthy workers', { count: terminated });
    }

    // Log final stats
    const stats = scheduler.getStats();
    const workerStats = workerManager.getStats();
    log('info', 'Daemon shutdown complete', {
      nodesExecuted: stats.nodesExecuted,
      nodesBlocked: stats.nodesBlocked,
      avgExecutionTimeMs: stats.avgExecutionTimeMs,
      workers: workerStats,
    });

    process.exit(0);
  };

  process.on('SIGINT', () => shutdown('SIGINT'));
  process.on('SIGTERM', () => shutdown('SIGTERM'));

  // Periodic stats logging
  setInterval(() => {
    if (shuttingDown) return;
    const stats = scheduler.getStats();
    const health = workerManager.getHealthSummary();
    log('info', 'Daemon stats', {
      nodesExecuted: stats.nodesExecuted,
      nodesPending: stats.nodesPending,
      nodesBlocked: stats.nodesBlocked,
      budgetUsed: stats.budgetUsedPercentage,
      readyQueue: scheduler.getReadyQueueLength(),
      executing: scheduler.getExecutingCount(),
      workerHealth: health,
    });
  }, 30000);

  // Start the scheduler
  await scheduler.start();
}

// Entry point
const config = parseDaemonArgs();
startDaemon(config).catch((error) => {
  log('fatal', 'Daemon crashed', { error: error instanceof Error ? error.message : String(error) });
  process.exit(1);
});
