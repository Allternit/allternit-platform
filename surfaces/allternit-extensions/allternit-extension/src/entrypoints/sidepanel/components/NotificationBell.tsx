import { useEffect, useState } from 'react'

import {
  getNotifications,
  getUnreadCount,
  markAllRead,
  onNotificationsChange,
  type ExtensionNotification,
} from '@/lib/notification-service'

export function NotificationBell() {
  const [unread, setUnread] = useState(0)
  const [isOpen, setIsOpen] = useState(false)
  const [notifications, setNotifications] = useState<ExtensionNotification[]>([])

  useEffect(() => {
    void getUnreadCount().then(setUnread)
    void getNotifications().then(setNotifications)

    const unsubscribe = onNotificationsChange((list) => {
      setNotifications(list)
      setUnread(list.filter((n) => !n.read).length)
    })
    return unsubscribe
  }, [])

  async function handleOpen() {
    if (!isOpen && unread > 0) {
      await markAllRead()
    }
    setIsOpen(!isOpen)
  }

  const levelIcons: Record<string, string> = {
    success: '✓',
    error: '✕',
    warning: '⚠',
    info: 'ℹ',
  }

  const levelColors: Record<string, string> = {
    success: 'text-emerald-500',
    error: 'text-red-500',
    warning: 'text-amber-500',
    info: 'text-blue-500',
  }

  return (
    <div className="relative">
      <button
        onClick={handleOpen}
        className="relative inline-flex size-7 items-center justify-center rounded-md text-[var(--text-primary,#2A1F16)]/60 transition-colors hover:bg-[var(--bg-secondary,#F5EDE3)] hover:text-[var(--text-primary,#2A1F16)]"
      >
        <svg className="size-4" viewBox="0 0 16 16" fill="currentColor">
          <path d="M8 16a2 2 0 0 0 2-2H6a2 2 0 0 0 2 2zm.995-14.901a1 1 0 1 0-1.99 0A5.002 5.002 0 0 0 3 6c0 1.098-.5 6-2 7h14c-1.5-1-2-5.902-2-7 0-2.42-1.72-4.44-4.005-4.901z" />
        </svg>
        {unread > 0 && (
          <span className="absolute -right-0.5 -top-0.5 flex size-4 items-center justify-center rounded-full bg-red-500 text-[9px] font-bold text-white">
            {unread > 9 ? '9+' : unread}
          </span>
        )}
      </button>

      {isOpen && (
        <div className="absolute right-0 top-full z-50 mt-2 w-72 max-h-80 overflow-y-auto rounded-lg border border-[var(--accent-primary,#1F1E1D)]/20 bg-[var(--bg-primary,#FDF8F3)] shadow-xl">
          <div className="sticky top-0 border-b border-[var(--accent-primary,#1F1E1D)]/10 bg-[var(--bg-primary,#FDF8F3)] px-3 py-2">
            <span className="text-xs font-semibold text-[var(--text-primary,#2A1F16)]">
              Notifications
            </span>
          </div>

          {notifications.length === 0 ? (
            <div className="px-3 py-6 text-center text-xs text-[var(--text-primary,#2A1F16)]/50">
              No notifications yet
            </div>
          ) : (
            <div className="divide-y divide-[var(--accent-primary,#1F1E1D)]/5">
              {notifications.slice(0, 20).map((n) => (
                <div
                  key={n.id}
                  className={`px-3 py-2 ${!n.read ? 'bg-[var(--bg-secondary,#F5EDE3)]/50' : ''}`}
                >
                  <div className="flex items-start gap-2">
                    <span className={`mt-0.5 text-xs ${levelColors[n.level]}`}>
                      {levelIcons[n.level]}
                    </span>
                    <div className="min-w-0 flex-1">
                      <p className="text-xs font-medium text-[var(--text-primary,#2A1F16)]">
                        {n.title}
                      </p>
                      <p className="mt-0.5 text-[10px] text-[var(--text-primary,#2A1F16)]/70">
                        {n.message}
                      </p>
                      <p className="mt-1 text-[9px] text-[var(--text-primary,#2A1F16)]/40">
                        {new Date(n.timestamp).toLocaleString()}
                      </p>
                    </div>
                  </div>
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  )
}
