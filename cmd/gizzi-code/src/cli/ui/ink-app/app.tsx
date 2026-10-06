#!/usr/bin/env bun
// @ts-nocheck

/**
 * Gizzi TUI Entry Point - Ink-based REPL initialization.
 */
import React from 'react'
import { createRoot } from './ink'
import { App } from './components/App'
import { REPL } from './screens/REPL'
import { enableConfigs } from './utils/config'
import { enableConfigs as enableSharedConfigs } from '../../../shared/utils/config'
import { getDefaultAppState } from './state/AppStateStore'
import { createStatsStore } from './context/stats'
import { getAllBaseTools } from './tools'
import { getCommands } from './commands'
import { createSystemMessage, createUserMessage } from './utils/messages'
import { getCliHighlightPromise } from './utils/cliHighlight'
import {
  initialPermissionModeFromCLI,
  initializeToolPermissionContext,
} from './utils/permissions/permissionSetup'
import {
  bypassRefusedAsRootMessage,
  tuiPermissionModeEnv,
} from './utils/permissions/tuiPermissionStartup'
import { setCwdState, setIsInteractive, setOriginalCwd, setSessionTrustAccepted, switchSession } from './bootstrap/state'
import { asSessionId } from './types/ids'
import { Log } from '../../../shared/util/log'
import { registerRailsPeer } from '../../../runtime/gizzi-core/services/railsPeer.js'
import { RailsInboxBridge } from './components/RailsInboxBridge'
import { RailsDagBridge } from './components/RailsDagBridge'
import { getSessionId } from './bootstrap/state.js'
import { routeInternalFetch } from './utils/routeInternalFetch'

export async function tui(options?: any): Promise<void> {
  const currentCwd = process.cwd()
  setOriginalCwd(currentCwd)
  setCwdState(currentCwd)
  setSessionTrustAccepted(true)
  // The TUI is the interactive session. Without this every
  // getIsNonInteractiveSession() check reads true (the state default) and
  // silently disables interactive-only features: /context, file history,
  // fork subagent, memory extraction, etc.
  setIsInteractive(true)

  // Honor -s/--session (e.g. `gizzi agents bot chat <name>` launches the TUI on the
  // bot's pinned canonical session): point the ink session id at the pinned
  // id up front so transcripts, session identity, and the Bot Mode composer
  // guard (D2: /new reroutes to compact) all see the same session id.
  if (options?.args?.sessionID) {
    switchSession(asSessionId(String(options.args.sessionID)))
  }

  enableConfigs()
  enableSharedConfigs()

  // Resolve the startup permission mode. thread.ts translates
  // --yolo / --dangerously-skip-permissions into env vars; without this
  // wiring the TUI always seeded mode 'default' and kept prompting.
  const { permissionModeCli, dangerouslySkipPermissions } =
    tuiPermissionModeEnv(process.env)
  if (dangerouslySkipPermissions || permissionModeCli === 'bypassPermissions') {
    const refusal = bypassRefusedAsRootMessage(process.env)
    if (refusal) {
      console.error(refusal)
      process.exit(1)
    }
  }
  const { mode: initialPermissionMode, notification: permissionModeNotification } =
    initialPermissionModeFromCLI({ permissionModeCli, dangerouslySkipPermissions })
  const permissionInit = await initializeToolPermissionContext({
    allowedToolsCli: [],
    disallowedToolsCli: [],
    permissionMode: initialPermissionMode,
    allowDangerouslySkipPermissions: dangerouslySkipPermissions,
    addDirs: [],
  })
  Log.Default.info('tui: permission mode resolved', {
    mode: initialPermissionMode,
    bypassAvailable: permissionInit.toolPermissionContext.isBypassPermissionsModeAvailable,
  })

  // Register as a Factory peer so other local agents can discover and message
  // this session. Fire-and-forget: failures are logged but never block TUI.
  // The actual inbox listener is mounted inside the React tree by
  // <RailsInboxBridge /> so it can post messages to the mailbox context.
  registerRailsPeer(getSessionId()).catch((err) => {
    Log.Default.info('tui: Factory peer registration failed', {
      error: err?.message || String(err),
    })
  })

  if (options?.fetch) {
    globalThis.fetch = routeInternalFetch(options.fetch, options.url, globalThis.fetch)
  }

  Log.Default.info("tui: getting tools and commands")
  // Warm the syntax highlighter now so the first streamed code block renders
  // colored instead of flashing plain → highlighted when the lazy import
  // resolves mid-stream.
  void getCliHighlightPromise()
  const initialTools = getAllBaseTools().filter((t: any) => t.isEnabled ? t.isEnabled() : true)
  const initialCommands = await getCommands(currentCwd)

  const startupNotices = [
    ...(permissionModeNotification ? [permissionModeNotification] : []),
    ...permissionInit.warnings,
  ]
  const initialMessages = [
    ...startupNotices.map(text => createSystemMessage(text, 'warning')),
    ...(options?.args?.prompt
      ? [createUserMessage({ content: options.args.prompt })]
      : []),
  ]

  const defaultState = getDefaultAppState()
  // Teammates spawned with plan_mode_required keep their forced plan mode.
  const forcedPlanMode = defaultState.toolPermissionContext.mode === 'plan'
  const initialState = {
    ...defaultState,
    toolPermissionContext: forcedPlanMode
      ? { ...permissionInit.toolPermissionContext, mode: 'plan' }
      : permissionInit.toolPermissionContext,
  }

  Log.Default.info("tui: rendering ink App & REPL via createRoot")
  try {
    const root = await createRoot({ exitOnCtrlC: false })
    root.render(
      <App
        initialState={initialState}
        stats={createStatsStore()}
        getFpsMetrics={() => undefined}
      >
        <>
          <REPL
            commands={initialCommands}
            debug={false}
            initialTools={initialTools}
            initialMessages={initialMessages}
            thinkingConfig={{ enabled: false, budgetTokens: 0 }}
          />
          <RailsInboxBridge />
          <RailsDagBridge />
        </>
      </App>
    )

    Log.Default.info("tui: waiting for exit")
    await root.waitUntilExit()
  } catch (err: any) {
    Log.Default.error("tui render exception", { error: err?.stack || err?.message || String(err) })
    console.error("TUI Render Error:", err)
  } finally {
    if (options?.onExit) {
      await options.onExit()
    }
  }
}

export async function startInkTUI(): Promise<void> {
  await tui()
}

if (import.meta.main) {
  startInkTUI().catch(err => {
    console.error('Failed to start TUI:', err)
    process.exit(1)
  })
}
