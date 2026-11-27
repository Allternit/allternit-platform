import { useEffect, useState } from 'react'

import type { HandoffRequest } from '@/browser-agent/safety/handoff'

const STORAGE_KEY = 'allternit_agent_handoffs'

const KIND_TEXT: Record<string, string> = {
	login: 'Sign in to continue',
	two_factor: 'Enter the verification code',
	captcha: 'Solve the captcha',
	block_page: 'The site is blocking the agent',
	rate_limited: 'The site says too many requests',
}

/** "Gizzi needs you · Continue · Stop" bar for a paused agent. */
export function NeedsYouBar() {
	const [pending, setPending] = useState<HandoffRequest | null>(null)

	useEffect(() => {
		const load = async () => {
			const result = await chrome.storage.local.get(STORAGE_KEY)
			const all = (result[STORAGE_KEY] as HandoffRequest[] | undefined) ?? []
			setPending(all.find((h) => !h.resolved && h.action.startsWith('needs_you:')) ?? null)
		}
		void load()
		const onChange = (changes: Record<string, chrome.storage.StorageChange>, area: string) => {
			if (area === 'local' && changes[STORAGE_KEY]) void load()
		}
		chrome.storage.onChanged.addListener(onChange)
		return () => chrome.storage.onChanged.removeListener(onChange)
	}, [])

	if (!pending) return null

	const kind = String(pending.details?.kind ?? '')
	const domain = String(pending.details?.domain ?? '')
	const answer = (approved: boolean) => {
		void chrome.runtime.sendMessage({
			type: 'AGENT_HANDOFF_RESOLVE',
			handoffId: pending.id,
			approved,
		})
	}

	return (
		<div
			role="alert"
			className="flex items-center gap-2 border-b bg-orange-50 px-3 py-2 text-sm text-orange-950"
		>
			<div className="min-w-0 flex-1">
				<div className="font-medium">Gizzi needs you</div>
				<div className="truncate text-xs">
					{KIND_TEXT[kind] ?? pending.reason}
					{domain ? ` on ${domain}` : ''}
				</div>
			</div>
			<button
				onClick={() => answer(true)}
				className="rounded-md bg-orange-600 px-2.5 py-1 text-xs font-medium text-white hover:bg-orange-700"
			>
				Continue
			</button>
			<button
				onClick={() => answer(false)}
				className="rounded-md border border-orange-300 px-2.5 py-1 text-xs font-medium hover:bg-orange-100"
			>
				Stop
			</button>
		</div>
	)
}
