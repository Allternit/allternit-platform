/**
 * Notebook Backend Process Manager
 * Manages SurrealDB + Open Notebook backend lifecycle without Docker.
 */

import { spawn, ChildProcess } from 'child_process';
import path from 'path';
import os from 'os';
import fs from 'fs';
import { PORTS, notebookUrl } from './config.js';

const DATA_DIR = path.join(os.homedir(), '.allternit', 'services', 'open-notebook');
const BIN_DIR = path.join(os.homedir(), '.allternit', 'bin');
const SURREAL_BIN = path.join(BIN_DIR, 'surreal');
const START_SCRIPT = path.join(DATA_DIR, 'start.sh');

export class NotebookManager {
  private surrealProcess?: ChildProcess;
  private backendProcess?: ChildProcess;
  private isReady = false;
  private healthCheckInterval?: NodeJS.Timeout;

  async start(): Promise<boolean> {
    console.log('[NotebookManager] Starting research backend...');

    // Ensure bootstrap has run
    if (!fs.existsSync(SURREAL_BIN)) {
      console.log('[NotebookManager] SurrealDB not found. Run bootstrap first.');
      return false;
    }

    // Start via the start script (manages both SurrealDB + FastAPI)
    this.backendProcess = spawn('bash', [START_SCRIPT], {
      detached: false,
      stdio: ['ignore', 'pipe', 'pipe'],
    });

    this.backendProcess.stdout?.on('data', (data) => {
      const line = data.toString().trim();
      if (line) console.log('[ON]', line);
      if (line.includes('Uvicorn running') || line.includes('Application startup complete')) {
        this.isReady = true;
      }
    });

    this.backendProcess.stderr?.on('data', (data) => {
      console.error('[ON]', data.toString().trim());
    });

    this.backendProcess.on('exit', (code) => {
      console.log(`[NotebookManager] Backend exited with code ${code}`);
      this.isReady = false;
    });

    // Wait for health check
    for (let i = 0; i < 60; i++) {
      await new Promise(r => setTimeout(r, 1000));
      if (this.isReady) {
        console.log(`[NotebookManager] Research backend ready on port ${PORTS.NOTEBOOK}`);
        this.startHealthCheck();
        return true;
      }
    }

    console.error('[NotebookManager] Backend failed to become ready');
    return false;
  }

  stop(): void {
    console.log('[NotebookManager] Stopping research backend...');
    if (this.healthCheckInterval) clearInterval(this.healthCheckInterval);
    if (this.backendProcess && !this.backendProcess.killed) {
      this.backendProcess.kill('SIGTERM');
    }
    this.isReady = false;
  }

  private startHealthCheck(): void {
    this.healthCheckInterval = setInterval(async () => {
      try {
        const res = await fetch(notebookUrl('/health'));
        if (!res.ok) throw new Error('Health check failed');
      } catch {
        console.warn('[NotebookManager] Health check failed, backend may need restart');
        this.isReady = false;
      }
    }, 30000);
  }

  getStatus(): { running: boolean; ready: boolean } {
    return {
      running: !!this.backendProcess && !this.backendProcess.killed,
      ready: this.isReady,
    };
  }
}

export const notebookManager = new NotebookManager();
