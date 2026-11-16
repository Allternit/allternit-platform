import { useState } from 'react'

import { exportSession, triggerDownload, type ExportFormat } from '@/lib/session-export'

interface SessionExportButtonProps {
  sessionId: string
  taskTitle: string
}

const FORMAT_OPTIONS: { value: ExportFormat; label: string }[] = [
  { value: 'json', label: 'JSON' },
  { value: 'markdown', label: 'Markdown' },
  { value: 'clipboard', label: 'Copy to clipboard' },
]

export function SessionExportButton({ sessionId, taskTitle }: SessionExportButtonProps) {
  const [isOpen, setIsOpen] = useState(false)
  const [exporting, setExporting] = useState(false)
  const [feedback, setFeedback] = useState<string | null>(null)

  async function handleExport(format: ExportFormat) {
    setExporting(true)
    setFeedback(null)

    try {
      const content = await exportSession(sessionId, { format, includeMetadata: true })
      const safeTitle = taskTitle.replace(/[^a-z0-9]+/gi, '-').slice(0, 40)

      if (format === 'clipboard') {
        setFeedback('Copied to clipboard')
      } else {
        const ext = format === 'json' ? 'json' : 'md'
        const mime = format === 'json' ? 'application/json' : 'text/markdown'
        triggerDownload(content, `allternit-${safeTitle}.${ext}`, mime)
        setFeedback('Download started')
      }
    } catch (error) {
      setFeedback(`Export failed: ${error instanceof Error ? error.message : 'unknown error'}`)
    } finally {
      setExporting(false)
      setIsOpen(false)
      setTimeout(() => setFeedback(null), 3000)
    }
  }

  return (
    <div className="relative">
      <button
        onClick={() => setIsOpen(!isOpen)}
        disabled={exporting}
        className="inline-flex items-center gap-1 rounded-md border border-[var(--accent-primary,#1F1E1D)]/30 px-2 py-1 text-xs text-[var(--text-primary,#2A1F16)] transition-colors hover:bg-[var(--bg-secondary,#F5EDE3)] disabled:opacity-50"
      >
        <svg className="size-3" viewBox="0 0 16 16" fill="currentColor">
          <path d="M8 1a.5.5 0 0 1 .5.5v7.793l2.146-2.147a.5.5 0 0 1 .708.708l-3 3a.5.5 0 0 1-.708 0l-3-3a.5.5 0 1 1 .708-.708L7.5 9.293V1.5A.5.5 0 0 1 8 1zM2 13.5a.5.5 0 0 1 .5-.5h11a.5.5 0 0 1 0 1h-11a.5.5 0 0 1-.5-.5z" />
        </svg>
        Export
      </button>

      {isOpen && (
        <div className="absolute right-0 top-full z-50 mt-1 w-40 overflow-hidden rounded-md border border-[var(--accent-primary,#1F1E1D)]/20 bg-[var(--bg-primary,#FDF8F3)] shadow-lg">
          {FORMAT_OPTIONS.map((opt) => (
            <button
              key={opt.value}
              onClick={() => handleExport(opt.value)}
              disabled={exporting}
              className="block w-full px-3 py-1.5 text-left text-xs text-[var(--text-primary,#2A1F16)] transition-colors hover:bg-[var(--bg-secondary,#F5EDE3)] disabled:opacity-50"
            >
              {opt.label}
            </button>
          ))}
        </div>
      )}

      {feedback && (
        <span className="absolute left-0 top-full mt-1 text-[10px] text-[var(--accent-primary,#1F1E1D)]">
          {feedback}
        </span>
      )}
    </div>
  )
}
