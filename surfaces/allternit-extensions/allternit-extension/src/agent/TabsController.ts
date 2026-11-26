import { handleTabControlMessage } from './TabsController.background'

const PREFIX = '[TabsController]'

function debug(...messages: any[]) {
	console.debug(`\x1b[90m${PREFIX}\x1b[0m`, ...messages)
}

function sendMessage(message: {
	type: 'TAB_CONTROL'
	action: TabAction
	payload?: any
}): Promise<any> {
	// A service worker does not receive its own runtime messages, so call the
	// background handler directly when the agent runs there (remote tasks).
	if (typeof window === 'undefined') {
		return new Promise((resolve) => {
			handleTabControlMessage(
				message as Parameters<typeof handleTabControlMessage>[0],
				{} as chrome.runtime.MessageSender,
				resolve
			)
		})
	}
	return chrome.runtime.sendMessage(message).catch((error) => {
		console.error(PREFIX, message.action, error)
		return null
	})
}

/**
 * Controller for managing browser tabs.
 * - live in the agent env (extension page or content script)
 * - no chrome apis. call sw for tab operations
 * - stateful across multiple execute() calls: tabs are reused between tasks
 */
export class TabsController extends EventTarget {
	currentTabId: number | null = null

	private tabs: TabMeta[] = []
	private initialTabId: number | null = null
	private tabGroupId: number | null = null
	private task: string = ''
	private windowId: number | null = null

	/** Remote tasks: work in their own unfocused window, tabs in one orange group. */
	ownWindow = false
	/** Bot name used in the tab group title ("<bot> · working"). */
	botName = 'Allternit'

	constructor() {
		super()

		// Set up the tab-change listener once for the lifetime of this controller.
		// init() may be called multiple times (one per execute()); keeping the
		// listener here prevents duplicate registrations.
		const tabChangeHandler = (message: any): void => {
			if (message.type !== 'TAB_CHANGE') return

			if (message.action === 'created') {
				const tab = message.payload.tab as chrome.tabs.Tab
				if (tab.groupId === this.tabGroupId && tab.id != null) {
					if (!this.tabs.find((t) => t.id === tab.id)) {
						this.tabs.push({ id: tab.id, isInitial: false })
					}
					this.switchToTab(tab.id)
				}
			} else if (message.action === 'removed') {
				const { tabId } = message.payload as { tabId: number }
				const targetTab = this.tabs.find((t) => t.id === tabId)
				if (targetTab) {
					this.tabs = this.tabs.filter((t) => t.id !== tabId)
					if (this.currentTabId === tabId) {
						const next = this.tabs[this.tabs.length - 1] || null
						if (next) {
							this.switchToTab(next.id)
						} else {
							this.updateCurrentTabId(null)
						}
					}
				}
			} else if (message.action === 'updated') {
				const { tabId, tab } = message.payload as { tabId: number; tab: chrome.tabs.Tab }
				const targetTab = this.tabs.find((t) => t.id === tabId)
				if (targetTab) {
					targetTab.url = tab.url
					targetTab.title = tab.title
					targetTab.status = tab.status
				}
			}
		}

		chrome.runtime.onMessage.addListener(tabChangeHandler)
		this.addEventListener('dispose', () => {
			chrome.runtime.onMessage.removeListener(tabChangeHandler)
		})
	}

	async init(task: string, includeInitialTab: boolean = true) {
		debug('init', task, includeInitialTab)
		this.task = task

		// Reuse existing tabs from a previous run — the agent continues in the
		// same tab group and picks up where it left off. Just refresh the tab
		// group label so the user sees the current task name.
		if (this.tabs.length > 0 && this.currentTabId != null) {
			debug('init: reusing existing tabs', this.tabs.map(t => t.id), 'current:', this.currentTabId)
			if (this.tabGroupId != null) {
				await sendMessage({
					type: 'TAB_CONTROL',
					action: 'update_tab_group',
					payload: {
						groupId: this.tabGroupId,
						properties: { title: this.groupTitle() },
					},
				}).catch(() => {
					// Tab group may have been closed by user — ignore
				})
			}
			return
		}

		if (this.ownWindow) {
			await this.initOwnWindow()
			return
		}

		// First run: full initialisation
		this.tabs = []
		this.currentTabId = null
		this.tabGroupId = null
		this.initialTabId = null

		const result = await sendMessage({
			type: 'TAB_CONTROL',
			action: 'get_active_tab',
		})

		this.initialTabId = result.tabId

		if (!this.initialTabId) {
			throw new Error('Failed to get initial tab ID')
		}

		if (includeInitialTab) {
			this.currentTabId = this.initialTabId

			const info = await sendMessage({
				type: 'TAB_CONTROL',
				action: 'get_tab_info',
				payload: { tabId: this.initialTabId },
			})

			this.tabs.push({
				id: result.tabId,
				isInitial: true,
				url: info.url,
				title: info.title,
				status: info.status,
			})

			await this.createTabGroup([this.initialTabId])
		}

		await this.updateCurrentTabId(this.currentTabId)
	}

	/** Open the agent's own window (not focused) and group its first tab. */
	private async initOwnWindow() {
		this.tabs = []
		this.currentTabId = null
		this.tabGroupId = null
		this.initialTabId = null

		const win = await sendMessage({ type: 'TAB_CONTROL', action: 'create_agent_window' })
		if (!win?.success) {
			throw new Error(`Failed to open the agent window: ${win?.error}`)
		}
		this.windowId = win.windowId as number
		this.initialTabId = win.tabId as number
		this.currentTabId = this.initialTabId
		this.tabs.push({ id: this.initialTabId, isInitial: true, url: 'about:blank', status: 'complete' })
		await this.createTabGroup([this.initialTabId])
		await this.updateCurrentTabId(this.currentTabId)
	}

	/** The title of the agent's tab group. */
	private groupTitle(): string {
		return this.ownWindow ? `${this.botName} · working` : `PageAgent(${this.task})`
	}

	/** Bring the agent window to the front so the person can act (login, captcha). */
	async focusAgentWindow(): Promise<void> {
		if (this.windowId == null) return
		await sendMessage({ type: 'TAB_CONTROL', action: 'focus_agent_window' })
	}

	/** Let vault autofill run on the current tab (only after the person approved). */
	async approveAutofillForCurrentTab(): Promise<void> {
		if (this.currentTabId == null) return
		await sendMessage({
			type: 'TAB_CONTROL',
			action: 'approve_agent_autofill',
			payload: { tabId: this.currentTabId },
		})
	}

	/**
	 * Task ended: close the agent's window, unless the person took it over.
	 * Resets state so the next task opens a fresh window.
	 */
	async closeOwnWindow(): Promise<{ closed: boolean }> {
		if (!this.ownWindow || this.windowId == null) return { closed: false }
		const result = await sendMessage({ type: 'TAB_CONTROL', action: 'close_agent_window' })
		this.windowId = null
		this.tabs = []
		this.currentTabId = null
		this.tabGroupId = null
		this.initialTabId = null
		await chrome.storage.local.set({ currentTabId: null })
		return { closed: Boolean(result?.closed) }
	}

	async openNewTab(url: string): Promise<string> {
		debug('openNewTab', url)

		const result = await sendMessage({
			type: 'TAB_CONTROL',
			action: 'open_new_tab',
			payload: { url, windowId: this.windowId },
		})

		if (!result.success) {
			throw new Error(`Failed to open new tab: ${result.error}`)
		}

		const tabId = result.tabId as number

		this.tabs.push({
			id: tabId,
			isInitial: false,
		})

		await this.switchToTab(tabId)

		if (!this.tabGroupId) {
			await this.createTabGroup([tabId])
		} else {
			await sendMessage({
				type: 'TAB_CONTROL',
				action: 'add_tab_to_group',
				payload: { tabId: result.tabId, groupId: this.tabGroupId },
			})
		}

		await this.waitUntilTabLoaded(tabId)

		return `✅ Opened new tab ID ${tabId} with URL ${url}`
	}

	async switchToTab(tabId: number): Promise<string> {
		debug('switchToTab', tabId)

		const targetTab = this.tabs.find((t) => t.id === tabId)
		if (!targetTab) {
			throw new Error(`Tab ID ${tabId} not found in tab list.`)
		}

		await this.updateCurrentTabId(tabId)

		return `✅ Switched to tab ID ${tabId}.`
	}

	async closeTab(tabId: number): Promise<string> {
		debug('closeTab', tabId)

		const targetTab = this.tabs.find((t) => t.id === tabId)
		if (!targetTab) {
			throw new Error(`Tab ID ${tabId} not found in tab list.`)
		}
		if (targetTab.isInitial) {
			throw new Error(`Cannot close the initial tab ID ${tabId}.`)
		}

		const result = await sendMessage({
			type: 'TAB_CONTROL',
			action: 'close_tab',
			payload: { tabId },
		})

		if (result.success) {
			this.tabs = this.tabs.filter((t) => t.id !== tabId)
			if (this.currentTabId === tabId) {
				const newCurrentTab = this.tabs[this.tabs.length - 1] || null
				if (newCurrentTab) {
					await this.switchToTab(newCurrentTab.id)
				} else {
					await this.updateCurrentTabId(null)
				}
			}

			return `✅ Closed tab ID ${tabId}.`
		} else {
			throw new Error(`Failed to close tab ID ${tabId}: ${result.error}`)
		}
	}

	private async createTabGroup(tabIds: number[]) {
		const result = await sendMessage({
			type: 'TAB_CONTROL',
			action: 'create_tab_group',
			payload: { tabIds },
		})

		if (!result?.success) {
			throw new Error(`Failed to create tab group: ${result?.error}`)
		}

		this.tabGroupId = result.groupId as number

		await sendMessage({
			type: 'TAB_CONTROL',
			action: 'update_tab_group',
			payload: {
				groupId: this.tabGroupId,
				properties: {
					title: this.groupTitle(),
					color: this.ownWindow ? 'orange' : randomColor(),
					collapsed: false,
				},
			},
		})
	}

	async updateCurrentTabId(tabId: number | null) {
		debug('updateCurrentTabId', tabId)

		this.currentTabId = tabId
		await chrome.storage.local.set({ currentTabId: tabId })
	}

	async getTabInfo(tabId: number): Promise<{ title: string; url: string }> {
		// use cached tab info if available
		const tabMeta = this.tabs.find((t) => t.id === tabId)
		if (tabMeta && tabMeta.url && tabMeta.title) {
			return { title: tabMeta.title, url: tabMeta.url }
		}

		// otherwise, pull the latest tab info from the background script
		debug('getTabInfo: pulling from background script', tabId)
		const result = await sendMessage({
			type: 'TAB_CONTROL',
			action: 'get_tab_info',
			payload: { tabId },
		})

		if (tabMeta) {
			tabMeta.url = result.url
			tabMeta.title = result.title
		}

		return result
	}

	async summarizeTabs(): Promise<string> {
		const summaries = [`| Tab ID | URL | Title | Current |`, `|-----|-----|-----|-----|`]
		for (const tab of this.tabs) {
			const { title, url } = await this.getTabInfo(tab.id)
			summaries.push(
				`| ${tab.id} | ${url} | ${title} | ${this.currentTabId === tab.id ? '✅' : ''} |`
			)
		}
		if (!this.tabs.length) {
			summaries.push('\nNo tabs available. Open a tab if needed.')
		}

		return summaries.join('\n')
	}

	async waitUntilTabLoaded(tabId: number): Promise<void> {
		const tab = this.tabs.find((t) => t.id === tabId)
		if (!tab) throw new Error(`Tab ID ${tabId} not found in tab list.`)

		if (tab.status === 'unloaded') throw new Error(`Tab ID ${tabId} is unloaded.`)
		if (tab.status === 'complete') return

		debug('waitUntilTabLoaded', tabId)
		await waitUntil(() => tab.status === 'complete', 4_000)
	}

	dispose() {
		this.dispatchEvent(new Event('dispose'))
	}
}

export type TabAction =
	| 'get_active_tab'
	| 'get_tab_info'
	| 'open_new_tab'
	| 'create_tab_group'
	| 'update_tab_group'
	| 'add_tab_to_group'
	| 'close_tab'
	| 'get_tab_title'
	| 'create_agent_window'
	| 'close_agent_window'
	| 'focus_agent_window'
	| 'approve_agent_autofill'

interface TabMeta {
	id: number
	isInitial: boolean
	url?: string
	title?: string
	status?: 'loading' | 'unloaded' | 'complete'
}

const TAB_GROUP_COLORS = ['blue', 'red', 'yellow', 'green', 'pink', 'purple', 'cyan'] as const

type TabGroupColor = (typeof TAB_GROUP_COLORS)[number]

function randomColor(): TabGroupColor {
	return TAB_GROUP_COLORS[Math.floor(Math.random() * TAB_GROUP_COLORS.length)]
}

/**
 * Wait until condition becomes true
 * @returns Returns when condition becomes true, throws otherwise
 * @param timeoutMS Timeout in milliseconds, default 1 minutes, throws error on timeout
 * @param error Error object to reject on timeout. If not provided, will resolve with false
 */
export async function waitUntil(
	check: () => boolean | Promise<boolean>,
	timeoutMS = 60_000,
	error?: string
): Promise<boolean> {
	if (await check()) return true

	return new Promise((resolve, reject) => {
		const start = Date.now()
		const poll = async () => {
			if (await check()) return resolve(true)
			if (Date.now() - start > timeoutMS) {
				if (error) {
					return reject(new Error(error))
				} else {
					return resolve(false)
				}
			}
			setTimeout(poll, 100)
		}
		setTimeout(poll, 100)
	})
}
