/**
 * AppshotViewer — renders an Appshot in a scrollable, interactive view.
 *
 * Features:
 * - Captured page content rendered in a sandboxed iframe
 * - Agent conversation replay with step-through controls
 * - Zoom and pan controls for the page snapshot
 * - Tool call timeline with expandable details
 */

import * as React from 'react'
import {
	ChevronDown,
	ChevronLeft,
	ChevronRight,
	ChevronUp,
	Clock,
	Code2,
	MessageSquare,
	Minus,
	Play,
	Plus,
	Reset,
	Zap,
} from 'lucide-react'

import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import type { Appshot, CapturedMessage, CapturedToolCall } from '@/lib/appshots/types'

// ── Palette tokens ────────────────────────────────────────────────────────────

const palette = {
	bgPrimary: '#FDF8F3',
	bgSecondary: '#F5EDE3',
	accentPrimary: '#1F1E1D',
	textPrimary: '#2A1F16',
	textSecondary: '#6B5B4E',
	border: '#D9C9B8',
	agentBubble: '#E8D5C4',
	userBubble: '#F0E6D9',
} as const

// ── Zoom levels ───────────────────────────────────────────────────────────────

const ZOOM_LEVELS = [0.25, 0.5, 0.75, 1, 1.25, 1.5, 2]
const DEFAULT_ZOOM = 0.75

// ── Component ─────────────────────────────────────────────────────────────────

export function AppshotViewer({
	appshot,
	onBack,
}: {
	appshot: Appshot
	onBack: () => void
}) {
	const [activeTab, setActiveTab] = React.useState<'snapshot' | 'agent' | 'tools'>('snapshot')
	const [zoom, setZoom] = React.useState(DEFAULT_ZOOM)
	const [panOffset, setPanOffset] = React.useState({ x: 0, y: 0 })
	const [isPanning, setIsPanning] = React.useState(false)
	const [panStart, setPanStart] = React.useState({ x: 0, y: 0 })
	const [replayStep, setReplayStep] = React.useState(0)
	const [isReplaying, setIsReplaying] = React.useState(false)

	const snapshotRef = React.useRef<HTMLDivElement>(null)
	const replayTimerRef = React.useRef<ReturnType<typeof setInterval> | null>(null)

	const messages = appshot.agentContext?.messages ?? []
	const toolCalls = appshot.agentContext?.toolCalls ?? []
	const hasAgentData = messages.length > 0 || toolCalls.length > 0

	// ── Replay logic ────────────────────────────────────────────────────

	const totalSteps = messages.length + toolCalls.length

	const allSteps = React.useMemo(() => {
		const combined: Array<
			| { kind: 'message'; data: CapturedMessage; timestamp: string }
			| { kind: 'tool'; data: CapturedToolCall; timestamp: string }
		> = []
		for (const m of messages) combined.push({ kind: 'message', data: m, timestamp: m.timestamp })
		for (const t of toolCalls) combined.push({ kind: 'tool', data: t, timestamp: t.timestamp })
		combined.sort((a, b) => a.timestamp.localeCompare(b.timestamp))
		return combined
	}, [messages, toolCalls])

	const startReplay = React.useCallback(() => {
		setReplayStep(0)
		setIsReplaying(true)
		setActiveTab('agent')
	}, [])

	React.useEffect(() => {
		if (!isReplaying) {
			if (replayTimerRef.current) clearInterval(replayTimerRef.current)
			return
		}

		replayTimerRef.current = setInterval(() => {
			setReplayStep((prev) => {
				if (prev >= allSteps.length - 1) {
					setIsReplaying(false)
					return prev
				}
				return prev + 1
			})
		}, 1200)

		return () => {
			if (replayTimerRef.current) clearInterval(replayTimerRef.current)
		}
	}, [isReplaying, allSteps.length])

	// ── Zoom / Pan ──────────────────────────────────────────────────────

	const zoomIn = () => {
		const idx = ZOOM_LEVELS.findIndex((z) => z > zoom)
		if (idx !== -1) setZoom(ZOOM_LEVELS[idx])
	}

	const zoomOut = () => {
		const idx = [...ZOOM_LEVELS].reverse().findIndex((z) => z < zoom)
		if (idx !== -1) setZoom(ZOOM_LEVELS[ZOOM_LEVELS.length - 1 - idx])
	}

	const resetView = () => {
		setZoom(DEFAULT_ZOOM)
		setPanOffset({ x: 0, y: 0 })
	}

	const handleMouseDown = (e: React.MouseEvent) => {
		if (e.button !== 0) return
		setIsPanning(true)
		setPanStart({ x: e.clientX - panOffset.x, y: e.clientY - panOffset.y })
	}

	const handleMouseMove = (e: React.MouseEvent) => {
		if (!isPanning) return
		setPanOffset({
			x: e.clientX - panStart.x,
			y: e.clientY - panStart.y,
		})
	}

	const handleMouseUp = () => {
		setIsPanning(false)
	}

	// ── Render ──────────────────────────────────────────────────────────

	return (
		<div
			data-slot="appshot-viewer"
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
				<div className="flex-1 min-w-0">
					<p className="text-xs font-semibold truncate">{appshot.title}</p>
					<p className="text-[10px] truncate" style={{ color: palette.textSecondary }}>
						{appshot.pageUrl}
					</p>
				</div>
				{hasAgentData && (
					<Button
						variant="outline"
						size="sm"
						onClick={startReplay}
						className="cursor-pointer h-7 px-2 text-xs gap-1"
						style={{
							borderColor: palette.accentPrimary,
							color: palette.accentPrimary,
							background: 'transparent',
						}}
					>
						<Play className="size-3" />
						Replay
					</Button>
				)}
			</header>

			{/* Tabs */}
			<nav
				className="flex border-b"
				style={{ borderColor: palette.border }}
			>
				<TabButton
					active={activeTab === 'snapshot'}
					onClick={() => setActiveTab('snapshot')}
					icon={<Code2 className="size-3" />}
					label="Snapshot"
				/>
				{hasAgentData && (
					<>
						<TabButton
							active={activeTab === 'agent'}
							onClick={() => setActiveTab('agent')}
							icon={<MessageSquare className="size-3" />}
							label={`Agent (${messages.length})`}
						/>
						<TabButton
							active={activeTab === 'tools'}
							onClick={() => setActiveTab('tools')}
							icon={<Zap className="size-3" />}
							label={`Tools (${toolCalls.length})`}
						/>
					</>
				)}
			</nav>

			{/* Content area */}
			<div className="flex-1 overflow-hidden relative">
				{activeTab === 'snapshot' && (
					<SnapshotView
						ref={snapshotRef}
						html={appshot.capturedHtml}
						zoom={zoom}
						panOffset={panOffset}
						onMouseDown={handleMouseDown}
						onMouseMove={handleMouseMove}
						onMouseUp={handleMouseUp}
					/>
				)}

				{activeTab === 'agent' && (
					<AgentReplayView
						messages={messages}
						visibleCount={isReplaying || replayStep > 0 ? replayStep + 1 : messages.length}
						isReplaying={isReplaying}
					/>
				)}

				{activeTab === 'tools' && <ToolTimelineView toolCalls={toolCalls} />}
			</div>

			{/* Replay controls */}
			{(isReplaying || replayStep > 0) && activeTab === 'agent' && (
				<ReplayControls
					step={replayStep}
					total={allSteps.length}
					isPlaying={isReplaying}
					onStepBack={() => setReplayStep((s) => Math.max(0, s - 1))}
					onStepForward={() => setReplayStep((s) => Math.min(allSteps.length - 1, s + 1))}
					onTogglePlay={() => setIsReplaying((p) => !p)}
					onReset={() => {
						setReplayStep(0)
						setIsReplaying(false)
					}}
				/>
			)}

			{/* Zoom controls (snapshot tab only) */}
			{activeTab === 'snapshot' && (
				<div
					className="flex items-center justify-center gap-1.5 py-1.5"
					style={{ borderTop: `1px solid ${palette.border}` }}
				>
					<button
						type="button"
						onClick={zoomOut}
						className="p-1 rounded cursor-pointer transition-colors"
						style={{ color: palette.textSecondary }}
						title="Zoom out"
					>
						<Minus className="size-3.5" />
					</button>
					<span
						className="text-[10px] font-mono w-10 text-center"
						style={{ color: palette.textSecondary }}
					>
						{Math.round(zoom * 100)}%
					</span>
					<button
						type="button"
						onClick={zoomIn}
						className="p-1 rounded cursor-pointer transition-colors"
						style={{ color: palette.textSecondary }}
						title="Zoom in"
					>
						<Plus className="size-3.5" />
					</button>
					<button
						type="button"
						onClick={resetView}
						className="p-1 rounded cursor-pointer transition-colors"
						style={{ color: palette.textSecondary }}
						title="Reset view"
					>
						<Reset className="size-3.5" />
					</button>
				</div>
			)}
		</div>
	)
}

// ── Tab button ────────────────────────────────────────────────────────────────

function TabButton({
	active,
	onClick,
	icon,
	label,
}: {
	active: boolean
	onClick: () => void
	icon: React.ReactNode
	label: string
}) {
	return (
		<button
			type="button"
			onClick={onClick}
			className="flex items-center gap-1.5 px-3 py-1.5 text-[11px] font-medium transition-colors cursor-pointer border-b-2"
			style={{
				color: active ? palette.accentPrimary : palette.textSecondary,
				borderColor: active ? palette.accentPrimary : 'transparent',
				background: 'transparent',
			}}
		>
			{icon}
			{label}
		</button>
	)
}

// ── Snapshot view ─────────────────────────────────────────────────────────────

const SnapshotView = React.forwardRef<
	HTMLDivElement,
	{
		html: string
		zoom: number
		panOffset: { x: number; y: number }
		onMouseDown: (e: React.MouseEvent) => void
		onMouseMove: (e: React.MouseEvent) => void
		onMouseUp: () => void
	}
>(({ html, zoom, panOffset, onMouseDown, onMouseMove, onMouseUp }, ref) => {
	return (
		<div
			ref={ref}
			data-slot="appshot-snapshot-view"
			className="w-full h-full overflow-hidden"
			style={{
				cursor: 'grab',
				background: '#f0ebe5',
			}}
			onMouseDown={onMouseDown}
			onMouseMove={onMouseMove}
			onMouseUp={onMouseUp}
			onMouseLeave={onMouseUp}
		>
			<div
				style={{
					transform: `translate(${panOffset.x}px, ${panOffset.y}px) scale(${zoom})`,
					transformOrigin: '0 0',
					transition: 'transform 0.15s ease-out',
					width: `${100 / zoom}%`,
					height: `${100 / zoom}%`,
				}}
			>
				<iframe
					sandbox="allow-same-origin"
					srcDoc={html}
					title="Appshot page snapshot"
					className="w-full h-full border-none"
					style={{ pointerEvents: 'none' }}
				/>
			</div>
		</div>
	)
})
SnapshotView.displayName = 'SnapshotView'

// ── Agent replay view ─────────────────────────────────────────────────────────

function AgentReplayView({
	messages,
	visibleCount,
	isReplaying,
}: {
	messages: CapturedMessage[]
	visibleCount: number
	isReplaying: boolean
}) {
	const scrollRef = React.useRef<HTMLDivElement>(null)
	const visibleMessages = messages.slice(0, visibleCount)

	// Auto-scroll to bottom during replay
	React.useEffect(() => {
		if (isReplaying && scrollRef.current) {
			scrollRef.current.scrollTop = scrollRef.current.scrollHeight
		}
	}, [visibleMessages.length, isReplaying])

	if (messages.length === 0) {
		return (
			<div className="flex items-center justify-center h-full text-xs" style={{ color: palette.textSecondary }}>
				No agent messages captured
			</div>
		)
	}

	return (
		<div ref={scrollRef} className="h-full overflow-y-auto p-3 space-y-2">
			{visibleMessages.map((msg, i) => (
				<MessageBubble
					key={i}
					message={msg}
					isVisible={true}
					isLatest={i === visibleMessages.length - 1 && isReplaying}
				/>
			))}
			{visibleMessages.length === 0 && (
				<div className="flex items-center justify-center h-full text-xs" style={{ color: palette.textSecondary }}>
					{isReplaying ? 'Starting replay…' : 'No messages to display'}
				</div>
			)}
		</div>
	)
}

function MessageBubble({
	message,
	isVisible,
	isLatest,
}: {
	message: CapturedMessage
	isVisible: boolean
	isLatest: boolean
}) {
	const [expanded, setExpanded] = React.useState(false)
	const isUser = message.role === 'user'
	const isSystem = message.role === 'system'

	const maxLen = 300
	const isLong = message.content.length > maxLen
	const displayContent = expanded || !isLong ? message.content : `${message.content.slice(0, maxLen)}…`

	return (
		<div
			className={cn(
				'flex transition-opacity duration-300',
				isUser ? 'justify-end' : 'justify-start',
				!isVisible && 'opacity-0',
			)}
		>
			<div
				className="max-w-[85%] rounded-lg px-3 py-2"
				style={{
					background: isUser ? palette.userBubble : isSystem ? palette.bgSecondary : palette.agentBubble,
					border: `1px solid ${palette.border}`,
					boxShadow: isLatest ? `0 0 0 2px ${palette.accentPrimary}40` : undefined,
				}}
			>
				<div
					className="text-[9px] font-semibold uppercase tracking-wider mb-0.5"
					style={{ color: palette.accentPrimary }}
				>
					{message.role}
				</div>
				<p className="text-xs leading-relaxed whitespace-pre-wrap">{displayContent}</p>
				{isLong && (
					<button
						type="button"
						onClick={() => setExpanded(!expanded)}
						className="text-[10px] mt-1 cursor-pointer flex items-center gap-0.5"
						style={{ color: palette.accentPrimary }}
					>
						{expanded ? <ChevronUp className="size-2.5" /> : <ChevronDown className="size-2.5" />}
						{expanded ? 'Show less' : 'Show more'}
					</button>
				)}
				<div className="text-[9px] mt-1" style={{ color: palette.textSecondary }}>
					{new Date(message.timestamp).toLocaleTimeString()}
				</div>
			</div>
		</div>
	)
}

// ── Tool timeline view ────────────────────────────────────────────────────────

function ToolTimelineView({ toolCalls }: { toolCalls: CapturedToolCall[] }) {
	if (toolCalls.length === 0) {
		return (
			<div className="flex items-center justify-center h-full text-xs" style={{ color: palette.textSecondary }}>
				No tool calls captured
			</div>
		)
	}

	return (
		<div className="h-full overflow-y-auto p-3 space-y-2">
			{toolCalls.map((tc, i) => (
				<ToolCallCard key={i} toolCall={tc} index={i} />
			))}
		</div>
	)
}

function ToolCallCard({ toolCall, index }: { toolCall: CapturedToolCall; index: number }) {
	const [showInput, setShowInput] = React.useState(false)
	const [showOutput, setShowOutput] = React.useState(false)

	return (
		<div
			data-slot="appshot-tool-call"
			className="rounded-lg overflow-hidden"
			style={{ border: `1px solid ${palette.border}`, background: palette.bgSecondary }}
		>
			{/* Header */}
			<button
				type="button"
				onClick={() => setShowInput(!showInput)}
				className="w-full flex items-center gap-2 px-3 py-2 text-left cursor-pointer"
				style={{ background: 'transparent' }}
			>
				<span
					className="text-[9px] font-mono rounded px-1.5 py-0.5"
					style={{
						background: palette.accentPrimary,
						color: palette.bgPrimary,
					}}
				>
					#{index + 1}
				</span>
				<span className="text-xs font-semibold font-mono flex-1">{toolCall.name}</span>
				<Clock className="size-3" style={{ color: palette.textSecondary }} />
				<span className="text-[9px]" style={{ color: palette.textSecondary }}>
					{new Date(toolCall.timestamp).toLocaleTimeString()}
				</span>
				<ChevronDown
					className={cn('size-3 transition-transform', showInput && 'rotate-180')}
					style={{ color: palette.textSecondary }}
				/>
			</button>

			{/* Input */}
			{showInput && (
				<div className="px-3 pb-2" style={{ borderTop: `1px solid ${palette.border}` }}>
					<p className="text-[10px] font-semibold mt-2 mb-1" style={{ color: palette.textSecondary }}>
						Input
					</p>
					<pre
						className="text-[10px] font-mono rounded p-2 overflow-x-auto whitespace-pre-wrap break-all"
						style={{
							background: palette.bgPrimary,
							border: `1px solid ${palette.border}`,
							color: palette.textPrimary,
							maxHeight: 200,
							overflow: 'auto',
						}}
					>
						{JSON.stringify(toolCall.input, null, 2)}
					</pre>

					{/* Output toggle */}
					{toolCall.output && (
						<>
							<button
								type="button"
								onClick={() => setShowOutput(!showOutput)}
								className="text-[10px] mt-2 cursor-pointer flex items-center gap-0.5"
								style={{ color: palette.accentPrimary }}
							>
								{showOutput ? <ChevronUp className="size-2.5" /> : <ChevronDown className="size-2.5" />}
								{showOutput ? 'Hide output' : 'Show output'}
							</button>
							{showOutput && (
								<pre
									className="text-[10px] font-mono rounded p-2 mt-1 overflow-x-auto whitespace-pre-wrap break-all"
									style={{
										background: palette.bgPrimary,
										border: `1px solid ${palette.border}`,
										color: palette.textPrimary,
										maxHeight: 200,
										overflow: 'auto',
									}}
								>
									{toolCall.output}
								</pre>
							)}
						</>
					)}
				</div>
			)}
		</div>
	)
}

// ── Replay controls ───────────────────────────────────────────────────────────

function ReplayControls({
	step,
	total,
	isPlaying,
	onStepBack,
	onStepForward,
	onTogglePlay,
	onReset,
}: {
	step: number
	total: number
	isPlaying: boolean
	onStepBack: () => void
	onStepForward: () => void
	onTogglePlay: () => void
	onReset: () => void
}) {
	return (
		<div
			className="flex items-center justify-center gap-2 py-2"
			style={{ borderTop: `1px solid ${palette.border}` }}
		>
			<button
				type="button"
				onClick={onReset}
				className="p-1 rounded cursor-pointer"
				style={{ color: palette.textSecondary }}
				title="Reset"
			>
				<Reset className="size-3.5" />
			</button>
			<button
				type="button"
				onClick={onStepBack}
				disabled={step === 0}
				className="p-1 rounded cursor-pointer disabled:opacity-30"
				style={{ color: palette.textSecondary }}
				title="Previous step"
			>
				<ChevronLeft className="size-4" />
			</button>
			<button
				type="button"
				onClick={onTogglePlay}
				className="p-1.5 rounded-full cursor-pointer"
				style={{
					background: palette.accentPrimary,
					color: palette.bgPrimary,
				}}
				title={isPlaying ? 'Pause' : 'Play'}
			>
				{isPlaying ? (
					<div className="flex gap-0.5">
						<div className="w-1 h-3 rounded-sm" style={{ background: palette.bgPrimary }} />
						<div className="w-1 h-3 rounded-sm" style={{ background: palette.bgPrimary }} />
					</div>
				) : (
					<Play className="size-3.5" />
				)}
			</button>
			<button
				type="button"
				onClick={onStepForward}
				disabled={step >= total - 1}
				className="p-1 rounded cursor-pointer disabled:opacity-30"
				style={{ color: palette.textSecondary }}
				title="Next step"
			>
				<ChevronRight className="size-4" />
			</button>
			<span className="text-[10px] font-mono ml-1" style={{ color: palette.textSecondary }}>
				{step + 1} / {total}
			</span>
		</div>
	)
}
