// @ts-nocheck
/**
 * Language servers the TUI uses with no plugin installed, so diagnostics after
 * an edit work out of the box (Claude Code/OpenCode parity).
 *
 * Only servers already on this machine are listed: a binary on PATH or in the
 * project's node_modules/.bin. TypeScript also falls back to
 * `bun x typescript-language-server` when the project has `typescript`, the
 * same fallback the engine uses (runtime/integrations/lsp/server.ts); set
 * GIZZI_DISABLE_LSP_DOWNLOAD=1 to skip that fetch. Nothing here starts a
 * process: the manager starts a server the first time a file it handles is
 * edited. Rust is left out on purpose (rust-analyzer runs `cargo check`).
 *
 * GIZZI_DISABLE_LSP_DIAGNOSTICS=1 turns all of this off.
 */
import { existsSync } from 'fs'
import { createRequire } from 'module'
import path from 'path'
import { getCwd } from '../../utils/cwd.js'
import { isEnvTruthy } from '../../utils/envUtils.js'
import { whichSync } from '../../utils/which.js'

export function lspDiagnosticsEnabled(): boolean {
  return !isEnvTruthy(process.env.GIZZI_DISABLE_LSP_DIAGNOSTICS)
}

function localBin(cwd: string, name: string): string | null {
  const bin = path.join(cwd, 'node_modules', '.bin', name)
  return existsSync(bin) ? bin : null
}

function findBin(cwd: string, name: string): string | null {
  return localBin(cwd, name) ?? whichSync(name)
}

function resolveTsserver(cwd: string): string | null {
  try {
    return createRequire(path.join(cwd, 'package.json')).resolve('typescript/lib/tsserver.js')
  } catch {
    return null
  }
}

const TS_LANGS: Record<string, string> = {
  '.ts': 'typescript',
  '.tsx': 'typescriptreact',
  '.mts': 'typescript',
  '.cts': 'typescript',
  '.js': 'javascript',
  '.jsx': 'javascriptreact',
  '.mjs': 'javascript',
  '.cjs': 'javascript',
}

export function getBuiltinLspServers(cwd: string = getCwd()): Record<string, any> {
  if (!lspDiagnosticsEnabled()) return {}
  const servers: Record<string, any> = {}

  const tsserver = resolveTsserver(cwd)
  const tsBin = findBin(cwd, 'typescript-language-server')
  const tsInit = tsserver ? { tsserver: { path: tsserver } } : {}
  if (tsBin) {
    servers['builtin:typescript'] = { command: tsBin, args: ['--stdio'], extensionToLanguage: TS_LANGS, initializationOptions: tsInit }
  } else if (tsserver && !isEnvTruthy(process.env.GIZZI_DISABLE_LSP_DOWNLOAD)) {
    servers['builtin:typescript'] = {
      command: process.execPath,
      args: ['x', 'typescript-language-server', '--stdio'],
      env: { BUN_BE_BUN: '1' },
      extensionToLanguage: TS_LANGS,
      initializationOptions: tsInit,
      // A first run fetches the server into bun's cache.
      startupTimeout: 60_000,
    }
  }

  const py = findBin(cwd, 'pyright-langserver') ?? findBin(cwd, 'basedpyright-langserver')
  if (py) servers['builtin:python'] = { command: py, args: ['--stdio'], extensionToLanguage: { '.py': 'python', '.pyi': 'python' } }

  const gopls = whichSync('gopls')
  if (gopls) servers['builtin:go'] = { command: gopls, extensionToLanguage: { '.go': 'go' } }

  for (const config of Object.values(servers)) {
    config.transport = 'stdio'
    config.workspaceFolder = cwd
  }
  return servers
}
