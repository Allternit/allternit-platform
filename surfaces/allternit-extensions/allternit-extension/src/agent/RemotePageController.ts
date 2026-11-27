import type { BrowserState } from '@page-agent/page-controller'

import { handlePageControlMessage } from './RemotePageController.background'
import type { TabsController } from './TabsController'

const PREFIX = '[RemotePageController]'

function debug(...messages: any[]) {
	console.debug(`\x1b[90m${PREFIX}\x1b[0m`, ...messages)
}

function sendMessage(message: {
	type: 'PAGE_CONTROL'
	action: string
	targetTabId: number
	payload?: any
}): Promise<any> {
	// A service worker does not receive its own runtime messages, so call the
	// background handler directly when the agent runs there (remote tasks).
	if (typeof window === 'undefined') {
		return new Promise((resolve) => {
			handlePageControlMessage(message, {} as chrome.runtime.MessageSender, resolve)
		})
	}
	return chrome.runtime.sendMessage(message).catch((error) => {
		console.error(PREFIX, message.action, error)
		return null
	})
}

/**
 * Agent side page controller.
 * - live in the agent env (extension page or content script)
 * - communicates with remote PageController via sw
 */
export class RemotePageController {
	tabsController: TabsController

	constructor(tabsController: TabsController) {
		this.tabsController = tabsController
	}

	get currentTabId(): number | null {
		return this.tabsController.currentTabId
	}

	private async getCurrentUrl(): Promise<string> {
		if (!this.currentTabId) return ''
		const { url } = await this.tabsController.getTabInfo(this.currentTabId)
		return url || ''
	}

	private async getCurrentTitle(): Promise<string> {
		if (!this.currentTabId) return ''
		const { title } = await this.tabsController.getTabInfo(this.currentTabId)
		return title || ''
	}

	async getLastUpdateTime(): Promise<number> {
		if (!this.currentTabId) return Date.now()
		return sendMessage({
			type: 'PAGE_CONTROL',
			action: 'get_last_update_time',
			targetTabId: this.currentTabId,
		})
	}

	async getBrowserState(): Promise<BrowserState> {
		if (!this.currentTabId) {
			return {
				url: '',
				title: '',
				header: 'No tabs open yet.',
				content: 'No tab is open. Use the open_new_tab tool to navigate to a website and begin the task.',
				footer: '',
			}
		}

		let browserState = {} as BrowserState
		debug('getBrowserState', this.currentTabId)

		const currentUrl = await this.getCurrentUrl()
		const currentTitle = await this.getCurrentTitle()

		if (!this.currentTabId || !isContentScriptAllowed(currentUrl)) {
			browserState = {
				url: currentUrl,
				title: currentTitle,
				header: '',
				content: '(empty page. either current page is not readable or not loaded yet.)',
				footer: '',
			}
		} else {
			browserState = await sendMessage({
				type: 'PAGE_CONTROL',
				action: 'get_browser_state',
				targetTabId: this.currentTabId,
			})
		}

		const sum = await this.tabsController.summarizeTabs()
		browserState.header = sum + '\n\n' + (browserState.header || '')

		debug('getBrowserState: success', this.currentTabId, browserState)

		return browserState
	}

	async updateTree(): Promise<void> {
		if (!this.currentTabId || !isContentScriptAllowed(await this.getCurrentUrl())) {
			return
		}

		await sendMessage({
			type: 'PAGE_CONTROL',
			action: 'update_tree',
			targetTabId: this.currentTabId,
		})
	}

	async cleanUpHighlights(): Promise<void> {
		if (!this.currentTabId || !isContentScriptAllowed(await this.getCurrentUrl())) {
			return
		}

		await sendMessage({
			type: 'PAGE_CONTROL',
			action: 'clean_up_highlights',
			targetTabId: this.currentTabId,
		})
	}

	async clickElement(...args: any[]): Promise<DomActionReturn> {
		const res = await this.remoteCallDomAction('click_element', args)
		// @note may cause page navigation, wait for 1 second to ensure the page loading started
		await new Promise((resolve) => setTimeout(resolve, 1000))
		return res
	}

	async inputText(...args: any[]): Promise<DomActionReturn> {
		return this.remoteCallDomAction('input_text', args)
	}

	async selectOption(...args: any[]): Promise<DomActionReturn> {
		return this.remoteCallDomAction('select_option', args)
	}

	async scroll(...args: any[]): Promise<DomActionReturn> {
		return this.remoteCallDomAction('scroll', args)
	}

	async scrollHorizontally(...args: any[]): Promise<DomActionReturn> {
		return this.remoteCallDomAction('scroll_horizontally', args)
	}

	async executeJavascript(...args: any[]): Promise<DomActionReturn> {
		return this.remoteCallDomAction('execute_javascript', args)
	}

	/** Tell the current tab's content script to show the mask and cursor. */
	async showMask(): Promise<void> {
		if (!this.currentTabId || !isContentScriptAllowed(await this.getCurrentUrl())) return
		await sendMessage({
			type: 'PAGE_CONTROL',
			action: 'show_mask',
			targetTabId: this.currentTabId,
			payload: [this.maskLabel],
		})
	}

	async hideMask(): Promise<void> {
		if (!this.currentTabId || !isContentScriptAllowed(await this.getCurrentUrl())) return
		await sendMessage({
			type: 'PAGE_CONTROL',
			action: 'hide_mask',
			targetTabId: this.currentTabId,
		})
	}

	/** Text next to the cursor, for example "Allternit · Opening the page". */
	private maskLabel = ''

	async setMaskLabel(label: string): Promise<void> {
		this.maskLabel = label
		await chrome.storage.local.set({ agentMaskLabel: label })
	}

	/** Ask the current page whether it needs the person (login, 2FA, captcha, block). */
	async detectBlocker(): Promise<{
		blocker: { kind: string; reason: string } | null
		url: string
		domain: string
	} | null> {
		if (!this.currentTabId || !isContentScriptAllowed(await this.getCurrentUrl())) return null
		return sendMessage({
			type: 'PAGE_CONTROL',
			action: 'detect_blocker',
			targetTabId: this.currentTabId,
		})
	}

	/** @note Managed by content script via storage polling. */
	dispose(): void {}

	private async remoteCallDomAction(action: string, payload: any[]): Promise<DomActionReturn> {
		if (!this.currentTabId) {
			return { success: false, message: 'RemotePageController not initialized.' }
		}

		if (!isContentScriptAllowed(await this.getCurrentUrl())) {
			return {
				success: false,
				message:
					'Operation not allowed on this page. Use open_new_tab to navigate to a web page first.',
			}
		}

		return sendMessage({
			type: 'PAGE_CONTROL',
			action: action,
			targetTabId: this.currentTabId!,
			payload,
		})
	}
}

interface DomActionReturn {
	success: boolean
	message: string
}

/**
 * Check if a URL can run content scripts.
 */
function isContentScriptAllowed(url: string | undefined): boolean {
	if (!url) return false

	const restrictedPatterns = [
		/^chrome:\/\//,
		/^chrome-extension:\/\//,
		/^about:/,
		/^edge:\/\//,
		/^brave:\/\//,
		/^opera:\/\//,
		/^vivaldi:\/\//,
		/^file:\/\//,
		/^view-source:/,
		/^devtools:\/\//,
	]

	return !restrictedPatterns.some((pattern) => pattern.test(url))
}
