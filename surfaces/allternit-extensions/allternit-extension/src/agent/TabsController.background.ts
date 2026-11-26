/**
 * background logics for TabsController
 */
import type { TabAction } from './TabsController'

const PREFIX = '[TabsController.background]'

function debug(...messages: any[]) {
	console.debug(`\x1b[90m${PREFIX}\x1b[0m`, ...messages)
}

/**
 * The agent's own window for a remote task. Lives in the service worker.
 * `tookOver` is set when the person focuses the window themselves.
 */
let agentWindow: { id: number; tookOver: boolean; expectFocus: boolean } | null = null
const autofillApprovedTabs = new Set<number>()

/** True when this tab lives in the agent's window. */
export function isAgentWindowTab(windowId: number | undefined): boolean {
	return agentWindow != null && windowId === agentWindow.id
}

/** Vault autofill in the agent's window runs only after the person approved. */
export function isAgentAutofillApproved(tabId: number): boolean {
	return autofillApprovedTabs.has(tabId)
}

export function handleTabControlMessage(
	message: { type: 'TAB_CONTROL'; action: TabAction; payload: any },
	sender: chrome.runtime.MessageSender,
	sendResponse: (response: unknown) => void
): true | undefined {
	const { action, payload } = message

	switch (action as TabAction) {
		case 'get_active_tab': {
			debug('get_active_tab')
			chrome.tabs
				.query({ active: true, currentWindow: true })
				.then((tabs) => {
					const tabId = tabs.length > 0 ? tabs[0].id || null : null
					debug('get_active_tab: success', tabId)
					sendResponse({ success: true, tabId })
				})
				.catch((error) => {
					sendResponse({ error: error instanceof Error ? error.message : String(error) })
				})
			return true // async response
		}

		case 'get_tab_info': {
			debug('get_tab_info', payload)
			chrome.tabs
				.get(payload.tabId)
				.then((tab) => {
					debug('get_tab_info: success', tab)
					sendResponse(tab)
				})
				.catch((error) => {
					sendResponse({ error: error instanceof Error ? error.message : String(error) })
				})
			return true // async response
		}

		case 'open_new_tab': {
			debug('open_new_tab', payload)
			chrome.tabs
				.create({
					url: payload.url,
					active: false,
					...(typeof payload.windowId === 'number' ? { windowId: payload.windowId } : {}),
				})
				.then((newTab) => {
					debug('open_new_tab: success', newTab)
					sendResponse({ success: true, tabId: newTab.id })
				})
				.catch((error) => {
					sendResponse({ error: error instanceof Error ? error.message : String(error) })
				})
			return true // async response
		}

		case 'create_tab_group': {
			debug('create_tab_group', payload)
			chrome.tabs
				.group({ tabIds: payload.tabIds })
				.then((groupId) => {
					debug('create_tab_group: success', groupId)
					sendResponse({ success: true, groupId })
				})
				.catch((error) => {
					console.error(PREFIX, 'Failed to create tab group', error)
					sendResponse({ error: error instanceof Error ? error.message : String(error) })
				})
			return true // async response
		}

		case 'update_tab_group': {
			debug('update_tab_group', payload)
			chrome.tabGroups
				.update(payload.groupId, payload.properties)
				.then(() => {
					sendResponse({ success: true })
				})
				.catch((error) => {
					sendResponse({ error: error instanceof Error ? error.message : String(error) })
				})
			return true // async response
		}

		case 'add_tab_to_group': {
			debug('add_tab_to_group', payload)
			chrome.tabs
				.group({ tabIds: payload.tabId, groupId: payload.groupId })
				.then(() => {
					sendResponse({ success: true })
				})
				.catch((error) => {
					sendResponse({ error: error instanceof Error ? error.message : String(error) })
				})
			return true // async response
		}

		case 'close_tab': {
			debug('close_tab', payload)
			chrome.tabs
				.remove(payload.tabId)
				.then(() => {
					sendResponse({ success: true })
				})
				.catch((error) => {
					sendResponse({ error: error instanceof Error ? error.message : String(error) })
				})
			return true // async response
		}

		case 'create_agent_window': {
			debug('create_agent_window')
			// Not focused, so it does not steal focus from what the person is doing.
			chrome.windows
				.create({ url: 'about:blank', focused: false, type: 'normal' })
				.then((win) => {
					const tabId = win.tabs?.[0]?.id
					if (win.id == null || tabId == null) throw new Error('Window has no tab')
					agentWindow = { id: win.id, tookOver: false, expectFocus: false }
					autofillApprovedTabs.clear()
					sendResponse({ success: true, windowId: win.id, tabId })
				})
				.catch((error) => {
					sendResponse({ error: error instanceof Error ? error.message : String(error) })
				})
			return true // async response
		}

		case 'focus_agent_window': {
			if (!agentWindow) {
				sendResponse({ success: false })
				return
			}
			// Our own focus is not the person taking over.
			agentWindow.expectFocus = true
			chrome.windows
				.update(agentWindow.id, { focused: true })
				.then(() => sendResponse({ success: true }))
				.catch((error) => {
					sendResponse({ error: error instanceof Error ? error.message : String(error) })
				})
			return true // async response
		}

		case 'approve_agent_autofill': {
			if (typeof payload?.tabId === 'number') autofillApprovedTabs.add(payload.tabId)
			sendResponse({ success: true })
			return
		}

		case 'close_agent_window': {
			const current = agentWindow
			agentWindow = null
			autofillApprovedTabs.clear()
			if (!current || current.tookOver) {
				// The person took the window over: leave it open for them.
				sendResponse({ success: true, closed: false })
				return
			}
			chrome.windows
				.remove(current.id)
				.then(() => sendResponse({ success: true, closed: true }))
				.catch(() => sendResponse({ success: true, closed: false }))
			return true // async response
		}

		default:
			sendResponse({ error: `Unknown action: ${action}` })
			return
	}
}

export function setupTabChangeEvents() {
	console.log('[TabsController.background] setupTabChangeEvents')

	// The person focusing the agent window themselves counts as taking over.
	chrome.windows?.onFocusChanged.addListener((windowId) => {
		if (!agentWindow || windowId !== agentWindow.id) return
		if (agentWindow.expectFocus) {
			agentWindow.expectFocus = false
			return
		}
		agentWindow.tookOver = true
	})
	chrome.windows?.onRemoved.addListener((windowId) => {
		if (agentWindow?.id === windowId) {
			agentWindow = null
			autofillApprovedTabs.clear()
		}
	})

	chrome.tabs.onCreated.addListener((tab) => {
		debug('onCreated', tab)
		chrome.runtime
			.sendMessage({ type: 'TAB_CHANGE', action: 'created', payload: { tab } })
			.catch((error) => {
				debug('onCreated error:', error)
			})
	})

	chrome.tabs.onRemoved.addListener((tabId, removeInfo) => {
		debug('onRemoved', tabId, removeInfo)
		chrome.runtime
			.sendMessage({
				type: 'TAB_CHANGE',
				action: 'removed',
				payload: { tabId, removeInfo },
			})
			.catch((error) => {
				debug('onRemoved error:', error)
			})
	})

	chrome.tabs.onUpdated.addListener((tabId, changeInfo, tab) => {
		debug('onUpdated', tabId, changeInfo)
		chrome.runtime
			.sendMessage({
				type: 'TAB_CHANGE',
				action: 'updated',
				payload: { tabId, changeInfo, tab },
			})
			.catch((error) => {
				debug('onUpdated error:', error)
			})
	})
}
