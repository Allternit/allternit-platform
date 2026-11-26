/**
 * RemoteTaskHandler
 *
 * Connects the page-agent extension to the Allternit Desktop app via native messaging.
 * The extension initiates a `chrome.runtime.connectNative('com.allternit.desktop')`
 * connection; Chrome launches the native host which bridges messages to the Desktop
 * app's TCP 3011 bridge (com.allternit.desktop native host).
 *
 * ── Inbound commands (Desktop → extension) ────────────────────────────────────
 *   { id, type: 'execute_task', payload: { task: string }, timestamp }
 *   { id, type: 'stop_task', payload: {}, timestamp }
 *   { id, type: 'get_status', payload: {}, timestamp }
 *   { id, type: 'resume_task', payload: { handoffId: string, approved: boolean }, timestamp }
 *       Answers a `needs_you` pause. approved=true continues, approved=false stops the task.
 *
 *   execute_task payload also takes `botName?: string` (tab group title and cursor label,
 *   default "Allternit"). The task runs in its own unfocused Chrome window with its tabs in
 *   one orange tab group titled "<bot> · working"; the window closes when the task ends
 *   unless the person took it over.
 *
 * ── Outbound events (extension → Desktop) ────────────────────────────────────
 *   { id, type: 'status',   payload: { status: AgentStatus }, timestamp }
 *   { id, type: 'activity', payload: AgentActivity, timestamp }
 *   { id, type: 'history',  payload: { events: HistoricalEvent[] }, timestamp }
 *   { id, type: 'done',     payload: { success: boolean; data: string }, timestamp }
 *   { id, type: 'error',    payload: { message: string }, timestamp }
 *   { id, type: 'pong',     payload: {}, timestamp }
 *   { id, type: 'needs_you', payload: { kind: 'login' | 'two_factor' | 'captcha' | 'block_page'
 *                                        | 'rate_limited', url, domain, handoffId }, timestamp }
 *       The agent is paused on this page until `resume_task` arrives.
 *
 * Native host name: com.allternit.desktop
 */

import type { AgentActivity, AgentStatus, HistoricalEvent } from '@page-agent/core'
import type { LLMConfig } from '@page-agent/llms'

import {
  createHandoff,
  resolveHandoff,
} from '@/browser-agent/safety/handoff'

import { MultiPageAgent, type NeedsYouInfo } from './MultiPageAgent'
import type { TabsController } from './TabsController'
import { DEMO_CONFIG, migrateLegacyEndpoint } from './constants'

// ── Types ─────────────────────────────────────────────────────────────────────

interface RemoteMessage {
  id: string
  type: string
  payload?: Record<string, unknown>
  timestamp: number
}

export interface PlatformTaskState {
  requestId: string
  status: AgentStatus
  task?: string
  activity?: AgentActivity | null
  history?: HistoricalEvent[]
  error?: string
}

// ── Constants ─────────────────────────────────────────────────────────────────

const NATIVE_HOST_NAME = 'com.allternit.desktop'
const RECONNECT_DELAY_MS = 3_000
const MAX_RECONNECT_DELAY_MS = 30_000

// ── RemoteTaskHandler ─────────────────────────────────────────────────────────

class RemoteTaskHandler {
  private port: chrome.runtime.Port | null = null
  private agent: MultiPageAgent | null = null
  private reconnectDelay = RECONNECT_DELAY_MS
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null
  private active = false
  /** Pending needs_you pauses: handoffId → resolver (true = continue, false = stop). */
  private pendingHandoffs = new Map<string, (approved: boolean) => void>()
  private platformStateListeners = new Set<(state: PlatformTaskState) => void>()

  /** Call once from background to begin managing the remote connection. */
  start(): void {
    if (this.active) return
    this.active = true
    this._connect()
  }

  /** Tear down — typically only called on extension unload. */
  stop(): void {
    this.active = false
    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer)
      this.reconnectTimer = null
    }
    this._disconnect()
    this._destroyAgent()
  }

  get isConnected(): boolean {
    return this.port !== null
  }

  requestPlatformTask(task: string, config?: Record<string, unknown>): string {
    const requestId = crypto.randomUUID()
    this._send({
      type: 'platform_task_request',
      payload: { requestId, task, config: config ?? {} },
    })
    return requestId
  }

  stopPlatformTask(requestId?: string): void {
    this._send({ type: 'platform_task_stop', payload: { requestId } })
  }

  onPlatformTaskState(listener: (state: PlatformTaskState) => void): () => void {
    this.platformStateListeners.add(listener)
    return () => this.platformStateListeners.delete(listener)
  }

  /**
   * Answer a needs_you pause (from Desktop's resume_task or the side panel bar).
   * Returns false when no such pause is waiting.
   */
  async resumeTask(handoffId: string, approved: boolean): Promise<boolean> {
    const waiting = this.pendingHandoffs.get(handoffId)
    await resolveHandoff(handoffId, approved).catch(() => null)
    if (!waiting) return false
    this.pendingHandoffs.delete(handoffId)
    waiting(approved)
    return true
  }

  // ── Private ────────────────────────────────────────────────────────────────

  private _connect(): void {
    if (!this.active) return

    try {
      this.port = chrome.runtime.connectNative(NATIVE_HOST_NAME)
    } catch (err) {
      console.warn('[RemoteTaskHandler] connectNative failed:', err)
      this._scheduleReconnect()
      return
    }

    console.log('[RemoteTaskHandler] Connected to', NATIVE_HOST_NAME)
    this.reconnectDelay = RECONNECT_DELAY_MS

    this.port.onMessage.addListener((msg: RemoteMessage) => this._handleMessage(msg))

    this.port.onDisconnect.addListener(() => {
      const err = chrome.runtime.lastError?.message ?? 'unknown reason'
      console.warn('[RemoteTaskHandler] Disconnected from native host:', err)
      this.port = null
      this._scheduleReconnect()
    })

    // Announce connection with current agent status
    this._send({ type: 'status', payload: { status: this.agent?.status ?? 'idle' } })
  }

  private _disconnect(): void {
    try {
      this.port?.disconnect()
    } catch {
      // ignore
    }
    this.port = null
  }

  private _scheduleReconnect(): void {
    if (!this.active) return
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null
      this._connect()
    }, this.reconnectDelay)
    this.reconnectDelay = Math.min(this.reconnectDelay * 2, MAX_RECONNECT_DELAY_MS)
  }

  private _send(partial: Omit<RemoteMessage, 'id' | 'timestamp'>): void {
    if (!this.port) return
    const msg: RemoteMessage = {
      id: crypto.randomUUID(),
      timestamp: Date.now(),
      ...partial,
    }
    try {
      this.port.postMessage(msg)
    } catch (err) {
      console.warn('[RemoteTaskHandler] postMessage failed:', err)
    }
  }

  // ── Message dispatch ───────────────────────────────────────────────────────

  private _handleMessage(msg: RemoteMessage): void {
    switch (msg.type) {
      case 'execute_task':
        this._onExecuteTask(msg)
        break
      case 'stop_task':
        this._onStopTask()
        break
      case 'get_status':
        this._send({ type: 'status', payload: { status: this.agent?.status ?? 'idle' } })
        break
      case 'resume_task':
        void this.resumeTask(
          String(msg.payload?.handoffId ?? ''),
          msg.payload?.approved === true,
        )
        break
      case 'configure':
        this._onConfigure(msg)
        break
      case 'ping':
        this._send({ type: 'pong', payload: {} })
        break
      case 'platform_task_state': {
        const state = msg.payload as unknown as PlatformTaskState
        if (state?.requestId) {
          this.platformStateListeners.forEach((listener) => listener(state))
        }
        break
      }
      default:
        console.warn('[RemoteTaskHandler] Unknown message type:', msg.type)
    }
  }

  private _onConfigure(msg: RemoteMessage): void {
    const {
      apiKey,
      baseURL,
      model,
      language,
      maxSteps,
      systemInstruction,
      experimentalLlmsTxt,
    } = msg.payload ?? {}

    const promises: Promise<void>[] = [
      chrome.storage.local.get(['llmConfig', 'advancedConfig']).then((result) => {
        const nextLlmConfig = {
          ...((result.llmConfig as Record<string, unknown> | undefined) ?? {}),
        }
        const nextAdvancedConfig = {
          ...((result.advancedConfig as Record<string, unknown> | undefined) ?? {}),
        }

        if (typeof apiKey === 'string') nextLlmConfig.apiKey = apiKey
        if (typeof baseURL === 'string') nextLlmConfig.baseURL = baseURL
        if (typeof model === 'string') nextLlmConfig.model = model

        if (typeof maxSteps === 'number') nextAdvancedConfig.maxSteps = maxSteps
        else if (maxSteps === null) delete nextAdvancedConfig.maxSteps

        if (typeof systemInstruction === 'string') nextAdvancedConfig.systemInstruction = systemInstruction
        else if (systemInstruction === null) delete nextAdvancedConfig.systemInstruction

        if (typeof experimentalLlmsTxt === 'boolean') {
          nextAdvancedConfig.experimentalLlmsTxt = experimentalLlmsTxt
        }

        const writes: Promise<void>[] = []
        if (Object.keys(nextLlmConfig).length > 0) {
          writes.push(chrome.storage.local.set({ llmConfig: nextLlmConfig }))
        }

        if (Object.keys(nextAdvancedConfig).length > 0) {
          writes.push(chrome.storage.local.set({ advancedConfig: nextAdvancedConfig }))
        } else {
          writes.push(chrome.storage.local.remove('advancedConfig'))
        }

        return Promise.all(writes).then(() => undefined)
      }),
    ]

    if (typeof language === 'string' && (language === 'en-US' || language === 'zh-CN')) {
      promises.push(chrome.storage.local.set({ language }))
    } else if (language === null) {
      promises.push(chrome.storage.local.remove('language'))
    }

    Promise.all(promises)
      .then(() => this._send({ type: 'configured', payload: {} }))
      .catch((err) => this._send({ type: 'error', payload: { message: String(err) } }))
  }

  private _onStopTask(): void {
    // A paused agent must wake up to stop.
    for (const id of Array.from(this.pendingHandoffs.keys())) void this.resumeTask(id, false)
    this.agent?.stop()
    this._send({ type: 'status', payload: { status: 'idle' } })
  }

  private async _onExecuteTask(msg: RemoteMessage): Promise<void> {
    const task = (msg.payload?.task as string | undefined) ?? ''
    if (!task) {
      this._send({ type: 'error', payload: { message: 'execute_task: task string is required' } })
      return
    }

    // If an agent is already running, stop it first
    if (this.agent && this.agent.status === 'running') {
      this.agent.stop()
      await new Promise<void>((res) => setTimeout(res, 200))
    }

    // Build a fresh agent for this task
    await this._destroyAgent()
    const agentConfig = await this._loadConfig()
    const botName =
      typeof msg.payload?.botName === 'string' && msg.payload.botName.trim()
        ? msg.payload.botName.trim()
        : 'Allternit'
    const agent = new MultiPageAgent({
      ...agentConfig,
      ownWindow: true,
      botName,
      onNeedsYou: (info, tabs) => this._needsYou(info, tabs),
    })
    this.agent = agent

    // Wire event listeners
    agent.addEventListener('statuschange', () => {
      this._send({ type: 'status', payload: { status: agent.status as AgentStatus } })
    })

    agent.addEventListener('historychange', () => {
      this._send({
        type: 'history',
        payload: { events: agent.history as unknown as Record<string, unknown>[] },
      })
    })

    agent.addEventListener('activity', (e: Event) => {
      const activity = (e as CustomEvent).detail as AgentActivity
      this._send({ type: 'activity', payload: activity as unknown as Record<string, unknown> })
      // Step text next to the cursor
      const step = describeStep(activity)
      if (step) {
        void agent.remotePageController.setMaskLabel(`${botName} · ${step}`)
      }
    })
    void agent.remotePageController.setMaskLabel(botName)

    // Execute and stream final result
    try {
      const result = await agent.execute(task)
      this._send({
        type: 'done',
        payload: { success: result.success, data: String(result.data ?? '') },
      })
    } catch (err) {
      this._send({ type: 'error', payload: { message: String(err) } })
    } finally {
      // Close the agent's window unless the person took it over.
      await agent.tabsController.closeOwnWindow().catch(() => undefined)
    }
  }

  /** Pause on a page that needs the person: record a handoff, tell Desktop, wait. */
  private async _needsYou(info: NeedsYouInfo, tabs: TabsController): Promise<boolean> {
    const handoff = await createHandoff(`needs_you:${info.kind}`, info.reason, {
      kind: info.kind,
      url: info.url,
      domain: info.domain,
    })
    const answer = new Promise<boolean>((resolve) => {
      this.pendingHandoffs.set(handoff.id, resolve)
    })
    this._send({
      type: 'needs_you',
      payload: { kind: info.kind, url: info.url, domain: info.domain, handoffId: handoff.id },
    })
    // Bring the agent's window forward so the person can act on the page.
    await tabs.focusAgentWindow().catch(() => undefined)

    const approved = await answer
    if (approved && info.kind === 'login') {
      // Vault autofill runs only now, after the person approved.
      await tabs.approveAutofillForCurrentTab().catch(() => undefined)
      if (tabs.currentTabId != null) {
        await chrome.tabs
          .sendMessage(tabs.currentTabId, { type: 'AUTOFILL_RESCAN' })
          .catch(() => undefined)
      }
    }
    return approved
  }

  private async _destroyAgent(): Promise<void> {
    if (!this.agent) return
    const agent = this.agent
    this.agent = null
    try {
      agent.dispose()
    } catch {
      // ignore
    }
  }

  /** Read LLM config from chrome.storage.local, same as useAgent hook. */
  private async _loadConfig(): Promise<{
    apiKey: string
    baseURL: string
    model: string
    language?: 'en-US' | 'zh-CN'
    maxSteps?: number
    experimentalLlmsTxt?: boolean
    instructions?: { system?: string }
  }> {
    const result = await chrome.storage.local.get(['llmConfig', 'language', 'advancedConfig'])

    let llmConfig: LLMConfig = (result.llmConfig as LLMConfig) ?? DEMO_CONFIG
    llmConfig = migrateLegacyEndpoint(llmConfig)

    const language = (result.language as 'en-US' | 'zh-CN') || undefined
    const advancedConfig = (
      result.advancedConfig as {
        maxSteps?: number
        systemInstruction?: string
        experimentalLlmsTxt?: boolean
      }
    ) ?? {}

    return {
      ...llmConfig,
      language,
      maxSteps: advancedConfig.maxSteps,
      experimentalLlmsTxt: advancedConfig.experimentalLlmsTxt,
      instructions: advancedConfig.systemInstruction
        ? { system: advancedConfig.systemInstruction }
        : undefined,
    }
  }
}

function describeStep(activity: AgentActivity): string {
  switch (activity.type) {
    case 'thinking':
      return 'Thinking'
    case 'executing':
      return activity.tool.replace(/_/g, ' ')
    case 'retrying':
      return 'Trying again'
    default:
      return ''
  }
}

// ── Singleton ─────────────────────────────────────────────────────────────────

export const remoteTaskHandler = new RemoteTaskHandler()
