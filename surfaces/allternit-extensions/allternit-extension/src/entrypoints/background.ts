/**
 * Allternit Extension — Background Service Worker
 *
 * Unified background for browser-agent + page-agent capabilities.
 *
 * Connection channels:
 *   Native (BA)  com.allternit.desktop  — browser-agent cowork mode (TCP 3011 bridge)
 *   Native (PA)  com.allternit.desktop  — page-agent remote task mode (TCP 3011 bridge)
 *
 * Populated by:
 *   MERGE-2  page-agent core (TabsController, RemotePageController, agent)
 *   MERGE-3  browser-agent connection layer (NativeMessaging)
 */

import { handlePageControlMessage } from '@/agent/RemotePageController.background'
import {
  handleTabControlMessage,
  isAgentAutofillApproved,
  isAgentWindowTab,
  setupTabChangeEvents,
} from '@/agent/TabsController.background'
import { remoteTaskHandler } from '@/agent/remote-task-handler'
import { browserAgentConnection } from '@/browser-agent/connection'
import { 
  handleHTMLToFigmaMessage, 
  setupHTMLToFigmaContextMenus,
  handleContextMenuClick 
} from '@/html-to-figma'
import {
  startCapture,
  stopCapture,
  isCaptureAvailable,
} from '@/api-capture/background'
import {
  findMatchingCredentials,
  fillCredential,
  recordCredentialUse,
} from '@/lib/vault/api'
import {
  isHistoryIngestionEnabled,
  recordBrowserVisit,
} from '@/lib/memory/history'
import { listLongRunningTasks, type LongRunningTask } from '@/lib/long-tasks/api'

const POLL_INTERVAL_MS = 15000
const LAST_TASKS_KEY = 'AllternitLongRunningTasksLastState'

function tasksEqual(a: LongRunningTask[], b: LongRunningTask[]): boolean {
  if (a.length !== b.length) return false
  const map = new Map(b.map((t) => [t.id, t.updated_at]))
  return a.every((t) => map.get(t.id) === t.updated_at)
}

export default defineBackground(() => {
  console.log('[Allternit Extension] Background Service Worker started')

  // ── Page-agent setup ──────────────────────────────────────────────────────

  setupTabChangeEvents()

  // ── Browser history as memory ─────────────────────────────────────────────

  if (chrome.history?.onVisited) {
    chrome.history.onVisited.addListener((item) => {
      isHistoryIngestionEnabled().then((enabled) => {
        if (!enabled) return
        recordBrowserVisit({
          url: item.url ?? '',
          title: item.title ?? undefined,
          visitTime: item.lastVisitTime ? new Date(item.lastVisitTime).toISOString() : undefined,
          transitionType: item.transition ?? undefined,
        }).catch((error) => {
          // Silent: history ingestion is best-effort and may fail when offline
          // or not yet authenticated.
          console.debug('[Allternit History] Failed to record visit:', error)
        })
      })
    })
  }

  // ── Long-running task polling ─────────────────────────────────────────────

  async function pollLongRunningTasks() {
    try {
      const tasks = await listLongRunningTasks('running')
      const stored = await chrome.storage.local.get(LAST_TASKS_KEY)
      const last: LongRunningTask[] = stored[LAST_TASKS_KEY] || []
      if (!tasksEqual(tasks, last)) {
        await chrome.storage.local.set({ [LAST_TASKS_KEY]: tasks })
        chrome.runtime
          .sendMessage({ type: 'LONG_RUNNING_TASKS_UPDATE', tasks })
          .catch(() => {})
      }
    } catch (error) {
      console.debug('[Allternit Long Tasks] Poll failed:', error)
    }
  }

  setInterval(pollLongRunningTasks, POLL_INTERVAL_MS)
  pollLongRunningTasks()

  chrome.storage.local.get('AllternitExtUserAuthToken').then((result) => {
    if (result.AllternitExtUserAuthToken) return
    chrome.storage.local.set({ AllternitExtUserAuthToken: crypto.randomUUID() })
  })

  // Side panel behavior
  chrome.sidePanel.setPanelBehavior({ openPanelOnActionClick: true }).catch(() => {})

  // Open the post-install onboarding click-through on first install
  chrome.runtime.onInstalled.addListener((details) => {
    if (details.reason !== 'install') return
    chrome.tabs.create({
      url: 'https://platform.allternit.com/extension/installed?source=install',
    })
  })

  // Page↔extension pairing: platform pages (whitelisted via externally_connectable)
  // request the bridge token; the content script activates the page-API bridge
  // when the page's localStorage token matches this value.
  chrome.runtime.onMessageExternal.addListener((message, _sender, sendResponse): true | undefined => {
    if (message?.type !== 'ALLTERNIT_PAIR_REQUEST') return undefined
    chrome.storage.local.get('AllternitExtUserAuthToken').then((result) => {
      sendResponse({ ok: true, token: result.AllternitExtUserAuthToken ?? null })
    })
    return true
  })

  // Remote task execution via com.allternit.desktop native messaging
  remoteTaskHandler.start()
  remoteTaskHandler.onPlatformTaskState((state) => {
    chrome.runtime.sendMessage({ type: 'PLATFORM_TASK_STATE', state }).catch(() => {})
  })

  // ── Browser-agent setup ───────────────────────────────────────────────────

  // Connects via com.allternit.desktop native messaging (cowork mode)
  browserAgentConnection.initialize()

  // ── HTML to Figma setup ───────────────────────────────────────────────────

  setupHTMLToFigmaContextMenus()

  chrome.contextMenus.onClicked.addListener((info, tab) => {
    handleContextMenuClick(info, tab)
  })

  // ── Message router ────────────────────────────────────────────────────────

  chrome.runtime.onMessage.addListener((message, sender, sendResponse): true | undefined => {
    // HTML to Figma messages
    if (message.type?.startsWith('HTML_TO_FIGMA')) {
      return handleHTMLToFigmaMessage(message, sender, sendResponse)
    }
    
    if (message.type === 'TAB_CONTROL') {
      return handleTabControlMessage(message, sender, sendResponse)
    }
    if (message.type === 'PAGE_CONTROL') {
      return handlePageControlMessage(message, sender, sendResponse)
    }
    if (message.type === 'NATIVE_HOST_STATUS') {
      browserAgentConnection
        .checkNativeHostStatus()
        .then((status) => {
          sendResponse({
            ok: true,
            extensionId: chrome.runtime.id,
            version: chrome.runtime.getManifest().version,
            status,
          })
        })
        .catch((error) => {
          sendResponse({
            ok: false,
            extensionId: chrome.runtime.id,
            error: error instanceof Error ? error.message : String(error),
          })
        })
      return true
    }
    if (message.type === 'PLATFORM_TASK_RUN') {
      const task = typeof message.task === 'string' ? message.task.trim() : ''
      if (!task) {
        sendResponse({ ok: false, error: 'Task is required' })
        return undefined
      }
      const requestId = remoteTaskHandler.requestPlatformTask(task, message.config)
      sendResponse({ ok: true, requestId })
      return undefined
    }
    if (message.type === 'PLATFORM_TASK_STOP') {
      remoteTaskHandler.stopPlatformTask(message.requestId)
      sendResponse({ ok: true })
      return undefined
    }
    // Side panel "Gizzi needs you · Continue · Stop" bar
    if (message.type === 'AGENT_HANDOFF_RESOLVE') {
      remoteTaskHandler
        .resumeTask(String(message.handoffId ?? ''), message.approved === true)
        .then((ok) => sendResponse({ ok }))
        .catch(() => sendResponse({ ok: false }))
      return true
    }
    if (message.type === 'PLATFORM_TASK_SUBSCRIBE') {
      sendResponse({ ok: true })
      return undefined
    }

    // Vault password-manager autofill messages
    if (message.type === 'AUTOFILL_REQUEST_CREDENTIALS') {
      const origin = message.payload?.origin as string | undefined
      if (!origin) {
        sendResponse({ ok: false, error: 'origin is required' })
        return undefined
      }
      findMatchingCredentials(origin)
        .then((credentials) => sendResponse({ ok: true, credentials }))
        .catch((error) =>
          sendResponse({
            ok: false,
            error: error instanceof Error ? error.message : String(error),
          }),
        )
      return true
    }
    if (message.type === 'AUTOFILL_FILL_CREDENTIAL') {
      const credentialId = message.payload?.credentialId as string | undefined
      const tabId = sender.tab?.id
      const origin =
        (message.payload?.origin as string | undefined) ??
        (sender.tab?.url ? new URL(sender.tab.url).hostname : undefined)
      if (!credentialId) {
        sendResponse({ ok: false, error: 'credentialId is required' })
        return undefined
      }
      if (typeof tabId !== 'number') {
        sendResponse({ ok: false, error: 'tabId is required' })
        return undefined
      }
      // The agent never fills a password on its own: in its window, autofill
      // waits until the person approved the pause (resume_task).
      if (isAgentWindowTab(sender.tab?.windowId) && !isAgentAutofillApproved(tabId)) {
        sendResponse({ ok: false, error: 'Waiting for you to approve before filling.' })
        return undefined
      }
      fillCredential(credentialId, origin)
        .then(async (filled) => {
          await chrome.tabs.sendMessage(tabId, {
            type: 'AUTOFILL_FILL_FIELDS',
            payload: {
              credentialId: filled.credential_id,
              username: filled.username,
              password: filled.password,
            },
          })
          sendResponse({ ok: true, credentialId: filled.credential_id })
        })
        .catch((error) =>
          sendResponse({
            ok: false,
            error: error instanceof Error ? error.message : String(error),
          }),
        )
      return true
    }
    if (message.type === 'AUTOFILL_RECORD_USE') {
      const credentialId = message.payload?.credentialId as string | undefined
      const origin =
        (message.payload?.origin as string | undefined) ??
        (sender.tab?.url ? new URL(sender.tab.url).hostname : undefined)
      if (!credentialId) {
        sendResponse({ ok: false, error: 'credentialId is required' })
        return undefined
      }
      recordCredentialUse(credentialId, origin)
        .then(() => sendResponse({ ok: true }))
        .catch((error) =>
          sendResponse({
            ok: false,
            error: error instanceof Error ? error.message : String(error),
          }),
        )
      return true
    }

    // API capture messages
    if (message.type === 'API_CAPTURE_AVAILABLE') {
      sendResponse({ ok: true, available: isCaptureAvailable() })
      return undefined
    }
    if (message.type === 'API_CAPTURE_START') {
      const tabId = message.tabId ?? sender.tab?.id
      if (!tabId) {
        sendResponse({ ok: false, error: 'No tab provided' })
        return undefined
      }
      startCapture(tabId, message.filterUrls)
        .then((result) => sendResponse({ ok: true, sessionId: result.sessionId }))
        .catch((error) => sendResponse({ ok: false, error: error instanceof Error ? error.message : String(error) }))
      return true
    }
    if (message.type === 'API_CAPTURE_STOP') {
      if (!message.sessionId) {
        sendResponse({ ok: false, error: 'sessionId is required' })
        return undefined
      }
      stopCapture(message.sessionId)
        .then((result) => sendResponse({ ok: true, har: result.har }))
        .catch((error) => sendResponse({ ok: false, error: error instanceof Error ? error.message : String(error) }))
      return true
    }
    // Browser-agent content script messages are handled by browserAgentConnection
    if (message.type === 'BROWSER_ACTION' || message.type === 'CONTENT_READY') {
      return browserAgentConnection.handleContentMessage(message, sender, sendResponse)
    }
    sendResponse({ error: 'Unknown message type' })
    return undefined
  })
})
