import { useEffect, useRef, useState } from 'react'

import { saveSession } from '@/lib/db'

import { ExtensionSidepanelShell } from '../../../../extension-shared/extension-sidepanel/ExtensionSidepanelShell'

import { ConfigPanel } from './components/ConfigPanel'
import { HistoryDetail } from './components/HistoryDetail'
import { HistoryList } from './components/HistoryList'
import { NeedsYouBar } from './components/NeedsYouBar'
import { useExtensionSidepanelAdapter } from './useExtensionSidepanelAdapter'
import { HTMLToFigmaPanel } from '@/html-to-figma/ui/HTMLToFigmaPanel'
import { CaptureComposer } from '@/html-to-figma/ui/CaptureComposer'
import { ResearchComposer } from './components/ResearchComposer'

const EXTENSION_SIDEPANEL_COPY = {
	title: 'Allternit Extension',
	subtitle: 'Chrome Sidepanel',
	emptyStateTitle: 'Allternit Extension',
	emptyStateDescription: 'Execute multi-page tasks',
	readyLabel: 'Ready',
	contextLabel: 'Current Browser Tab',
} as const

export default function App() {
	const { adapter, config, configure, currentTask, history, status } = useExtensionSidepanelAdapter()
	const [showHTMLToFigma, setShowHTMLToFigma] = useState(false)

	const prevStatusRef = useRef(status)
	useEffect(() => {
		const prev = prevStatusRef.current
		prevStatusRef.current = status

		if (
			prev === 'running' &&
			(status === 'completed' || status === 'error') &&
			history.length > 0 &&
			currentTask
		) {
			saveSession({ task: currentTask, history, status }).catch((err) =>
				console.error('[SidePanel] Failed to save session:', err)
			)
		}
	}, [status, history, currentTask])

	// Custom config panel wrapper that includes HTML→Figma
	const ConfigPanelWithHTMLToFigma = ({ onBack }: { onBack: () => void }) => {
		if (showHTMLToFigma) {
			return (
				<div className="h-full flex flex-col">
					<header className="flex items-center gap-2 border-b px-3 py-2">
						<button
							onClick={() => setShowHTMLToFigma(false)}
							className="inline-flex size-7 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
						>
							←
						</button>
						<span className="flex-1 text-sm font-medium">HTML to Figma</span>
					</header>
					<div className="flex-1 overflow-y-auto">
						<HTMLToFigmaPanel />
					</div>
				</div>
			)
		}

		return (
			<ConfigPanel
				config={config}
				onSave={async (newConfig) => {
					await configure(newConfig)
					onBack()
				}}
				onClose={onBack}
				onOpenHTMLToFigma={() => setShowHTMLToFigma(true)}
			/>
		)
	}

	return (
		<div className="flex h-full flex-col">
		<NeedsYouBar />
		<div className="min-h-0 flex-1">
		<ExtensionSidepanelShell
			adapter={adapter}
			copy={EXTENSION_SIDEPANEL_COPY}
			testId="extension-sidepanel-shell"
			renderConfigView={({ onBack }) => (
				<ConfigPanelWithHTMLToFigma onBack={onBack} />
			)}
			renderHistoryListView={({ onSelect, onBack }) => (
				<HistoryList onSelect={onSelect} onBack={onBack} />
			)}
			renderHistoryDetailView={({ sessionId, onBack }) => (
				<HistoryDetail sessionId={sessionId} onBack={onBack} />
			)}
			renderComposer={(props) => <ResearchComposer {...props} />}
		/>
		</div>
		</div>
	)
}
