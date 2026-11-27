import { type AgentConfig, PageAgentCore } from '@page-agent/core'

import { RemotePageController } from './RemotePageController'
import { TabsController } from './TabsController'
import SYSTEM_PROMPT from './system_prompt.md?raw'
import { createTabTools } from './tabTools'

/** Detect user language from browser settings */
function detectLanguage(): 'en-US' | 'zh-CN' {
	const lang = navigator.language || navigator.languages?.[0] || 'en-US'
	return lang.startsWith('zh') ? 'zh-CN' : 'en-US'
}

/**
 * MultiPageAgent
 * - use with extension
 * - can be used from a side panel or a content script
 */
export interface NeedsYouInfo {
	kind: 'login' | 'two_factor' | 'captcha' | 'block_page' | 'rate_limited'
	reason: string
	url: string
	domain: string
}

export interface MultiPageAgentOptions {
	includeInitialTab?: boolean
	/** Remote tasks: run in the agent's own window with an orange tab group. */
	ownWindow?: boolean
	/** Bot name for the tab group title and the cursor label. */
	botName?: string
	/**
	 * Called when the page needs the person (login, 2FA, captcha, block page,
	 * rate limit). The agent is paused until it resolves; false stops the task.
	 */
	onNeedsYou?: (info: NeedsYouInfo, tabs: TabsController) => Promise<boolean>
}

export class MultiPageAgent extends PageAgentCore {
	readonly tabsController: TabsController
	readonly remotePageController: RemotePageController

	constructor(config: AgentConfig & MultiPageAgentOptions) {
		// multi page controller
		const tabsController = new TabsController()
		tabsController.ownWindow = config.ownWindow ?? false
		tabsController.botName = config.botName || 'Allternit'
		const pageController = new RemotePageController(tabsController)
		const customTools = createTabTools(tabsController)

		// system prompt - auto-detect language if not specified
		const language = config.language ?? detectLanguage()
		const targetLanguage = language === 'zh-CN' ? '中文' : 'English'
		const systemPrompt = SYSTEM_PROMPT.replace(
			/Default working language: \*\*.*?\*\*/,
			`Default working language: **${targetLanguage}**`
		)

		// Do not include the initial tab by default — agent opens its own fresh tabs,
		// leaving the user's current working tab untouched.
		const includeInitialTab = config.ownWindow ? true : (config.includeInitialTab ?? false)

		/**
		 * When the agent is in side-panel and user closed the side-panel.
		 * There is no chance for isAgentRunning to be set false.
		 * (unload event doesn't work well in side panel.)
		 * (I'm trying not to use long-lived connection because the lifecycle of a sw is hard to predict.)
		 * This heartbeat mechanism acts as a backup.
		 */
		let heartBeatInterval: null | ReturnType<typeof setInterval> = null

		super({
			...config,
			pageController: pageController as any,
			customTools: customTools,
			customSystemPrompt: systemPrompt,

			onBeforeTask: async (agent) => {
				await tabsController.init(agent.task, includeInitialTab)

				heartBeatInterval = setInterval(() => {
					chrome.storage.local.set({
						agentHeartbeat: Date.now(),
					})
				}, 1_000)

				await chrome.storage.local.set({
					isAgentRunning: true,
				})
			},

			onAfterTask: async () => {
				if (heartBeatInterval) {
					clearInterval(heartBeatInterval)
					heartBeatInterval = null
				}

				await chrome.storage.local.set({
					isAgentRunning: false,
					agentPaused: false,
				})
			},

			onBeforeStep: async (agent) => {
				// make sure the current tab is loaded before the step starts
				// currentTabId may be null if no tab has been opened yet (includeInitialTab=false)
				if (tabsController.currentTabId != null) {
					await tabsController.waitUntilTabLoaded(tabsController.currentTabId)
				}

				// Pause for the person on login, 2FA, captcha, block and rate-limit pages.
				// The agent never types a password itself.
				if (config.onNeedsYou) {
					const found = await pageController.detectBlocker()
					if (found?.blocker) {
						await chrome.storage.local.set({ agentPaused: true })
						let approved = false
						try {
							approved = await config.onNeedsYou(
								{
									kind: found.blocker.kind as NeedsYouInfo['kind'],
									reason: found.blocker.reason,
									url: found.url,
									domain: found.domain,
								},
								tabsController
							)
						} finally {
							await chrome.storage.local.set({ agentPaused: false })
						}
						if (!approved) {
							throw new Error('Stopped: you chose to stop the task.')
						}
					}
				}
			},

			onDispose: () => {
				if (heartBeatInterval) {
					clearInterval(heartBeatInterval)
					heartBeatInterval = null
				}

				chrome.storage.local.set({
					isAgentRunning: false,
				})

				tabsController.dispose()
			},
		})

		this.tabsController = tabsController
		this.remotePageController = pageController
	}
}
