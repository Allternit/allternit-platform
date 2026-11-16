import { useCallback, useEffect, useRef, useState } from 'react'

interface QuickTaskOverlayProps {
  isOpen: boolean
  onClose: () => void
  onSubmit: (task: string) => void
}

const RECENT_TASKS_KEY = 'allternit-recent-tasks'
const MAX_RECENT = 5

export function QuickTaskOverlay({ isOpen, onClose, onSubmit }: QuickTaskOverlayProps) {
  const [task, setTask] = useState('')
  const [recentTasks, setRecentTasks] = useState<string[]>([])
  const inputRef = useRef<HTMLTextAreaElement>(null)

  useEffect(() => {
    if (isOpen) {
      loadRecentTasks()
      setTimeout(() => inputRef.current?.focus(), 50)
    } else {
      setTask('')
    }
  }, [isOpen])

  useEffect(() => {
    if (!isOpen) return

    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape') {
        onClose()
      } else if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
        handleSubmit()
      }
    }

    document.addEventListener('keydown', handleKeyDown)
    return () => document.removeEventListener('keydown', handleKeyDown)
  }, [isOpen, task])

  async function loadRecentTasks() {
    const result = await chrome.storage.local.get(RECENT_TASKS_KEY)
    setRecentTasks((result[RECENT_TASKS_KEY] as string[]) ?? [])
  }

  async function saveRecentTask(newTask: string) {
    const updated = [newTask, ...recentTasks.filter((t) => t !== newTask)].slice(0, MAX_RECENT)
    setRecentTasks(updated)
    await chrome.storage.local.set({ [RECENT_TASKS_KEY]: updated })
  }

  const handleSubmit = useCallback(() => {
    const trimmed = task.trim()
    if (!trimmed) return
    void saveRecentTask(trimmed)
    onSubmit(trimmed)
    onClose()
  }, [task, onSubmit, onClose])

  if (!isOpen) return null

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center bg-black/40 backdrop-blur-sm"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose()
      }}
    >
      <div className="mt-16 w-full max-w-md rounded-xl border border-[var(--accent-primary,#1F1E1D)]/20 bg-[var(--bg-primary,#FDF8F3)] p-4 shadow-2xl">
        <div className="mb-3 flex items-center justify-between">
          <h2 className="text-sm font-semibold text-[var(--text-primary,#2A1F16)]">
            Quick Task
          </h2>
          <button
            onClick={onClose}
            className="rounded p-1 text-[var(--text-primary,#2A1F16)]/50 transition-colors hover:bg-[var(--bg-secondary,#F5EDE3)] hover:text-[var(--text-primary,#2A1F16)]"
          >
            <svg className="size-4" viewBox="0 0 16 16" fill="currentColor">
              <path d="M4.646 4.646a.5.5 0 0 1 .708 0L8 7.293l2.646-2.647a.5.5 0 0 1 .708.708L8.707 8l2.647 2.646a.5.5 0 0 1-.708.708L8 8.707l-2.646 2.647a.5.5 0 0 1-.708-.708L7.293 8 4.646 5.354a.5.5 0 0 1 0-.708z" />
            </svg>
          </button>
        </div>

        <textarea
          ref={inputRef}
          value={task}
          onChange={(e) => setTask(e.target.value)}
          placeholder="What would you like Allternit to do?"
          rows={3}
          className="w-full resize-none rounded-lg border border-[var(--accent-primary,#1F1E1D)]/30 bg-white p-3 text-sm text-[var(--text-primary,#2A1F16)] placeholder:text-[var(--text-primary,#2A1F16)]/40 focus:border-[var(--accent-primary,#1F1E1D)] focus:outline-none focus:ring-2 focus:ring-[var(--accent-primary,#1F1E1D)]/20"
        />

        <div className="mt-2 flex items-center justify-between">
          <span className="text-[10px] text-[var(--text-primary,#2A1F16)]/50">
            ⌘/Ctrl+Enter to submit · Esc to close
          </span>
          <button
            onClick={handleSubmit}
            disabled={!task.trim()}
            className="rounded-md bg-[var(--accent-primary,#1F1E1D)] px-3 py-1.5 text-xs font-medium text-white transition-colors hover:bg-[var(--accent-primary,#1F1E1D)]/90 disabled:cursor-not-allowed disabled:opacity-50"
          >
            Run Task
          </button>
        </div>

        {recentTasks.length > 0 && (
          <div className="mt-3 border-t border-[var(--accent-primary,#1F1E1D)]/10 pt-2">
            <p className="mb-1.5 text-[10px] font-medium uppercase tracking-wider text-[var(--text-primary,#2A1F16)]/50">
              Recent
            </p>
            <div className="space-y-1">
              {recentTasks.map((recent, idx) => (
                <button
                  key={idx}
                  onClick={() => setTask(recent)}
                  className="block w-full truncate rounded px-2 py-1 text-left text-xs text-[var(--text-primary,#2A1F16)]/80 transition-colors hover:bg-[var(--bg-secondary,#F5EDE3)]"
                >
                  {recent}
                </button>
              ))}
            </div>
          </div>
        )}
      </div>
    </div>
  )
}
