/**
 * AppshotPanel — sidepanel component for listing and managing Appshots.
 *
 * Displays saved appshots with thumbnails, metadata, and action buttons
 * for sharing and exporting. Uses the Allternit sand/nude palette.
 */

import * as React from 'react'
import {
	Clock,
	Download,
	ImageOff,
	Share2,
	Trash2,
	Zap,
} from 'lucide-react'

import { Button } from '@/components/ui/button'
import {
	clearAppshots,
	deleteAppshot,
	exportAppshot,
	generateShareUrl,
	listAppshots,
	saveAppshot,
} from '@/lib/appshots/storage'
import type { Appshot } from '@/lib/appshots/types'

import { AppshotViewer } from './AppshotViewer'

// ── Palette tokens ────────────────────────────────────────────────────────────

const palette = {
	bgPrimary: '#FDF8F3',
	bgSecondary: '#F5EDE3',
	accentPrimary: '#1F1E1D',
	textPrimary: '#2A1F16',
	textSecondary: '#6B5B4E',
	border: '#D9C9B8',
} as const

// ── Helpers ───────────────────────────────────────────────────────────────────

function timeAgo(isoDate: string): string {
	const seconds = Math.floor((Date.now() - new Date(isoDate).getTime()) / 1000)
	if (seconds < 60) return 'just now'
	const minutes = Math.floor(seconds / 60)
	if (minutes < 60) return `${minutes}m ago`
	const hours = Math.floor(minutes / 60)
	if (hours < 24) return `${hours}h ago`
	const days = Math.floor(hours / 24)
	if (days < 30) return `${days}d ago`
	return new Date(isoDate).toLocaleDateString()
}

function truncate(str: string, max: number): string {
	return str.length > max ? `${str.slice(0, max)}…` : str
}

// ── Component ─────────────────────────────────────────────────────────────────

export function AppshotPanel({ onBack }: { onBack: () => void }) {
	const [appshots, setAppshots] = React.useState<Appshot[]>([])
	const [loading, setLoading] = React.useState(true)
	const [selectedId, setSelectedId] = React.useState<string | null>(null)
	const [copiedId, setCopiedId] = React.useState<string | null>(null)

	const load = React.useCallback(async () => {
		setAppshots(await listAppshots())
		setLoading(false)
	}, [])

	React.useEffect(() => {
		// eslint-disable-next-line react-hooks/set-state-in-effect
		load()
	}, [load])

	// ── Actions ─────────────────────────────────────────────────────────

	const handleDelete = async (e: React.MouseEvent, id: string) => {
		e.stopPropagation()
		await deleteAppshot(id)
		setAppshots((prev) => prev.filter((a) => a.id !== id))
		if (selectedId === id) setSelectedId(null)
	}

	const handleClearAll = async () => {
		await clearAppshots()
		setAppshots([])
		setSelectedId(null)
	}

	const handleExport = (e: React.MouseEvent, appshot: Appshot) => {
		e.stopPropagation()
		const html = exportAppshot(appshot)
		const blob = new Blob([html], { type: 'text/html' })
		const url = URL.createObjectURL(blob)
		const a = document.createElement('a')
		a.href = url
		a.download = `${appshot.title.replace(/[^a-zA-Z0-9]+/g, '-').toLowerCase()}.html`
		a.click()
		URL.revokeObjectURL(url)
	}

	const handleShare = async (e: React.MouseEvent, appshot: Appshot) => {
		e.stopPropagation()
		const url = generateShareUrl(appshot)

		// Update appshot with share URL if generated
		if (url !== appshot.shareUrl) {
			const updated = { ...appshot, shareUrl: url }
			await saveAppshot(updated)
			setAppshots((prev) => prev.map((a) => (a.id === appshot.id ? updated : a)))
		}

		try {
			await navigator.clipboard.writeText(url)
			setCopiedId(appshot.id)
			setTimeout(() => setCopiedId(null), 2000)
		} catch {
			// Clipboard API may not be available in sidepanel
		}
	}

	const handleCapture = async () => {
		// Send message to content script to capture current page
		try {
			const [tab] = await chrome.tabs.query({ active: true, currentWindow: true })
			if (!tab?.id) return

			const response = await chrome.tabs.sendMessage(tab.id, {
				type: 'APPSHOT_CAPTURE_PAGE',
			})

			if (response?.appshot) {
				const appshot = response.appshot as Appshot
				await saveAppshot(appshot)
				setAppshots((prev) => [appshot, ...prev])
			}
		} catch {
			// Content script may not be injected on all pages
		}
	}

	// ── Viewer mode ─────────────────────────────────────────────────────

	const selectedAppshot = appshots.find((a) => a.id === selectedId)

	if (selectedAppshot) {
		return (
			<AppshotViewer
				appshot={selectedAppshot}
				onBack={() => setSelectedId(null)}
			/>
		)
	}

	// ── List mode ───────────────────────────────────────────────────────

	return (
		<div
			data-slot="appshot-panel"
			className="flex flex-col h-screen"
			style={{ background: palette.bgPrimary, color: palette.textPrimary }}
		>
			{/* Header */}
			<header
				className="flex items-center gap-2 px-3 py-2"
				style={{ borderBottom: `1px solid ${palette.border}` }}
			>
				<Button
					variant="ghost"
					size="sm"
					onClick={onBack}
					className="cursor-pointer h-7 px-2 text-xs"
					style={{ color: palette.textSecondary }}
				>
					← Back
				</Button>
				<span className="text-sm font-semibold flex-1">Appshots</span>

				<Button
					variant="outline"
					size="sm"
					onClick={handleCapture}
					className="cursor-pointer h-7 px-2.5 text-xs gap-1.5"
					style={{
						borderColor: palette.accentPrimary,
						color: palette.accentPrimary,
						background: 'transparent',
					}}
				>
					<Zap className="size-3" />
					Capture
				</Button>

				{appshots.length > 0 && (
					<Button
						variant="ghost"
						size="sm"
						onClick={handleClearAll}
						className="cursor-pointer h-7 px-2 text-xs hover:text-red-600"
						style={{ color: palette.textSecondary }}
					>
						<Trash2 className="size-3 mr-1" />
						Clear
					</Button>
				)}
			</header>

			{/* Appshot list */}
			<div className="flex-1 overflow-y-auto">
				{loading && (
					<div
						className="flex items-center justify-center h-32 text-xs"
						style={{ color: palette.textSecondary }}
					>
						Loading appshots…
					</div>
				)}

				{!loading && appshots.length === 0 && (
					<div className="flex flex-col items-center justify-center h-48 gap-3 text-center px-6">
						<ImageOff className="size-8" style={{ color: palette.border }} />
						<div>
							<p className="text-sm font-medium" style={{ color: palette.textPrimary }}>
								No appshots yet
							</p>
							<p className="text-xs mt-1" style={{ color: palette.textSecondary }}>
								Click "Capture" to snapshot the current page
							</p>
						</div>
					</div>
				)}

				{appshots.map((appshot) => (
					<AppshotCard
						key={appshot.id}
						appshot={appshot}
						isCopied={copiedId === appshot.id}
						onSelect={() => setSelectedId(appshot.id)}
						onDelete={(e) => handleDelete(e, appshot.id)}
						onExport={(e) => handleExport(e, appshot)}
						onShare={(e) => handleShare(e, appshot)}
					/>
				))}
			</div>

			{/* Footer */}
			<footer
				className="text-center text-[10px] py-2"
				style={{ color: palette.textSecondary, borderTop: `1px solid ${palette.border}` }}
			>
				{appshots.length} appshot{appshots.length !== 1 ? 's' : ''} saved locally
			</footer>
		</div>
	)
}

// ── Card sub-component ────────────────────────────────────────────────────────

function AppshotCard({
	appshot,
	isCopied,
	onSelect,
	onDelete,
	onExport,
	onShare,
}: {
	appshot: Appshot
	isCopied: boolean
	onSelect: () => void
	onDelete: (e: React.MouseEvent) => void
	onExport: (e: React.MouseEvent) => void
	onShare: (e: React.MouseEvent) => void
}) {
	const messageCount = appshot.agentContext?.messages.length ?? 0
	const toolCount = appshot.agentContext?.toolCalls.length ?? 0

	return (
		<div
			data-slot="appshot-card"
			role="button"
			tabIndex={0}
			onClick={onSelect}
			onKeyDown={(e) => e.key === 'Enter' && onSelect()}
			className="group cursor-pointer transition-colors"
			style={{
				padding: '0.625rem 0.75rem',
				borderBottom: `1px solid ${palette.border}`,
			}}
			onMouseEnter={(e) => {
				;(e.currentTarget as HTMLElement).style.background = palette.bgSecondary
			}}
			onMouseLeave={(e) => {
				;(e.currentTarget as HTMLElement).style.background = 'transparent'
			}}
		>
			<div className="flex items-start gap-2.5">
				{/* Thumbnail */}
				<div
					className="shrink-0 rounded overflow-hidden"
					style={{
						width: 56,
						height: 35,
						background: palette.bgSecondary,
						border: `1px solid ${palette.border}`,
					}}
				>
					{appshot.thumbnail ? (
						<img
							src={appshot.thumbnail}
							alt=""
							className="w-full h-full object-cover"
						/>
					) : (
						<div className="w-full h-full flex items-center justify-center">
							<ImageOff className="size-3.5" style={{ color: palette.border }} />
						</div>
					)}
				</div>

				{/* Content */}
				<div className="flex-1 min-w-0">
					<p className="text-xs font-semibold truncate" style={{ color: palette.textPrimary }}>
						{truncate(appshot.title, 40)}
					</p>
					<p className="text-[10px] mt-0.5 truncate" style={{ color: palette.textSecondary }}>
						{appshot.pageUrl}
					</p>
					<div className="flex items-center gap-2 mt-1 text-[10px]" style={{ color: palette.textSecondary }}>
						<span className="flex items-center gap-0.5">
							<Clock className="size-2.5" />
							{timeAgo(appshot.createdAt)}
						</span>
						{messageCount > 0 && <span>{messageCount} msgs</span>}
						{toolCount > 0 && <span>{toolCount} tools</span>}
					</div>
				</div>

				{/* Actions (visible on hover) */}
				<div className="flex items-center gap-0.5 opacity-0 group-hover:opacity-100 transition-opacity shrink-0">
					<button
						type="button"
						onClick={onShare}
						title={isCopied ? 'Copied!' : 'Copy share link'}
						className="p-1 rounded transition-colors cursor-pointer"
						style={{ color: isCopied ? '#22c55e' : palette.accentPrimary }}
					>
						<Share2 className="size-3" />
					</button>
					<button
						type="button"
						onClick={onExport}
						title="Export as HTML"
						className="p-1 rounded transition-colors cursor-pointer"
						style={{ color: palette.accentPrimary }}
					>
						<Download className="size-3" />
					</button>
					<button
						type="button"
						onClick={onDelete}
						title="Delete"
						className="p-1 rounded transition-colors cursor-pointer hover:text-red-500"
						style={{ color: palette.textSecondary }}
					>
						<Trash2 className="size-3" />
					</button>
				</div>
			</div>

			{/* Agent badge */}
			{appshot.agentContext && (
				<div
					className="flex items-center gap-1.5 mt-1.5 ml-[68px] text-[9px] rounded px-1.5 py-0.5 w-fit"
					style={{
						background: `${palette.accentPrimary}15`,
						color: palette.accentPrimary,
					}}
				>
					<Zap className="size-2.5" />
					{appshot.agentContext.model}
					<span style={{ color: palette.textSecondary }}>
						· {appshot.agentContext.totalTokens.toLocaleString()} tokens
					</span>
				</div>
			)}
		</div>
	)
}
