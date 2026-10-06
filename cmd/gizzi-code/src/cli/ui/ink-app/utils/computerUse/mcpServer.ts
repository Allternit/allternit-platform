import {
  buildComputerUseTools,
  createComputerUseMcpServer,
} from './engine/index.js'
import type { Server } from '@modelcontextprotocol/server'
import { serveStdio } from '@modelcontextprotocol/server/stdio'
import { homedir } from 'os'

import { shutdownDatadog } from '../../services/analytics/datadog.js'
import { shutdown1PEventLogging } from '../../services/analytics/firstPartyEventLogger.js'
import { initializeAnalyticsSink } from '../../services/analytics/sink.js'
import { enableConfigs } from '../config.js'
import { logForDebugging } from '../debug.js'
import { filterAppsForDescription } from './appNames.js'
import { getChicagoCoordinateMode } from './gates.js'
import { getComputerUseHostAdapter } from './hostAdapter.js'

const APP_ENUM_TIMEOUT_MS = 1000

/**
 * Enumerate installed apps, timed. Fails soft — with the engine backend the
 * executor returns [] immediately (the engine has no app enumeration), so
 * the tool description just omits the list. Resolution happens at call time
 * regardless; the model just doesn't get hints.
 */
async function tryGetInstalledAppNames(): Promise<string[] | undefined> {
  const adapter = getComputerUseHostAdapter()
  const enumP = adapter.executor.listInstalledApps()
  let timer: ReturnType<typeof setTimeout> | undefined
  const timeoutP = new Promise<undefined>(resolve => {
    timer = setTimeout(resolve, APP_ENUM_TIMEOUT_MS, undefined)
  })
  const installed = await Promise.race([enumP, timeoutP])
    .catch(() => undefined)
    .finally(() => clearTimeout(timer))
  if (!installed) {
    // The enumeration continues in the background — swallow late rejections.
    void enumP.catch(() => {})
    logForDebugging(
      `[Computer Use MCP] app enumeration exceeded ${APP_ENUM_TIMEOUT_MS}ms or failed; tool description omits list`,
    )
    return undefined
  }
  return filterAppsForDescription(installed, homedir())
}

/**
 * Construct the in-process server. Delegates to the adapter's
 * `createComputerUseMcpServer` for the Server object + stub CallTool handler,
 * then REPLACES the ListTools handler with one that includes installed-app
 * names in the `request_access` description (the factory doesn't take
 * `installedAppNames`).
 *
 * Async so the 1s app-enumeration timeout doesn't block startup — called from
 * an `await import()` in `client.ts` on first CU connection, not `main.tsx`.
 *
 * Real dispatch still goes through `wrapper.tsx`'s `.call()` override; this
 * server exists only to answer ListTools.
 */
let installedAppNamesPromise: ReturnType<typeof tryGetInstalledAppNames> | undefined

export async function createComputerUseMcpServerForCli(): Promise<Server> {
  const adapter = getComputerUseHostAdapter()
  const coordinateMode = getChicagoCoordinateMode()
  const server = createComputerUseMcpServer(adapter, coordinateMode)

  // Memoized: the stdio entrypoint builds a server per 2026-07-28 request.
  installedAppNamesPromise ??= tryGetInstalledAppNames()
  const installedAppNames = await installedAppNamesPromise
  const tools = buildComputerUseTools(
    adapter.executor.capabilities,
    coordinateMode,
    installedAppNames,
  )
  server.setRequestHandler('tools/list', async () =>
    adapter.isDisabled() ? { tools: [] } : { tools },
  )

  return server
}

/**
 * Subprocess entrypoint for `--computer-use-mcp`. Mirror of
 * `runAllternitInChromeMcpServer` — stdio transport, exit on stdin close,
 * flush analytics before exit.
 */
export async function runComputerUseMcpServer(): Promise<void> {
  enableConfigs()
  initializeAnalyticsSink()

  let exiting = false
  const shutdownAndExit = async (): Promise<void> => {
    if (exiting) return
    exiting = true
    await Promise.all([shutdown1PEventLogging(), shutdownDatadog()])
    // eslint-disable-next-line custom-rules/no-process-exit
    process.exit(0)
  }
  process.stdin.on('end', () => void shutdownAndExit())
  process.stdin.on('error', () => void shutdownAndExit())

  logForDebugging('[Computer Use MCP] Starting MCP server')
  // Dual-era stdio: 2026-07-28 clients get stateless per-request servers from
  // the factory; 2025-era clients (initialize) are pinned to one instance.
  await createComputerUseMcpServerForCli()
  serveStdio(() => createComputerUseMcpServerForCli(), { legacy: 'serve' })
  logForDebugging('[Computer Use MCP] MCP server started')
}
