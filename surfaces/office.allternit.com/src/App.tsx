import { useEffect, useMemo, useState, useCallback } from 'react'
import { ClerkProvider, SignInButton, useAuth } from '@clerk/clerk-react'
import {
  OfficeHostProvider,
  createBrowserHost,
  createAllternitAssistantExtension,
  DocsApp,
  SheetsApp,
  SlidesApp,
  PdfApp,
  SignApp,
  OfficeAiSlot,
  type OfficeHost,
  type OpenOptions,
} from '@allternit/office-suite'
import { setOfficeAiTokenProvider } from '@allternit/office-ai'
import { createStandaloneAiClient } from './ai/createStandaloneAiClient'
import { CLERK_PUBLISHABLE_KEY } from './clerkConfig'
import { HomePage } from './HomePage'
import type { AppTab } from './types'

interface OpenedDoc {
  name: string
  bytes: Uint8Array
}

const TABS: { id: AppTab; label: string }[] = [
  { id: 'docs', label: 'Docs' },
  { id: 'sheets', label: 'Sheets' },
  { id: 'slides', label: 'Slides' },
  { id: 'pdf', label: 'PDF' },
  { id: 'sign', label: 'Sign' },
]

const ACCEPT_MAP: Record<AppTab, OpenOptions['accept']> = {
  docs: {
    'application/vnd.openxmlformats-officedocument.wordprocessingml.document': ['.docx'],
  },
  sheets: {
    'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet': ['.xlsx'],
  },
  slides: {
    'application/vnd.openxmlformats-officedocument.presentationml.presentation': ['.pptx'],
  },
  pdf: { 'application/pdf': ['.pdf'] },
  sign: { 'application/pdf': ['.pdf'] },
}

/**
 * Sign has no built-in chat surface of its own, so the extension slot is
 * rendered beside it in a horizontal row. `appKey="pdf"` — Sign operates on
 * PDFs, and the assistant's model override/context key off the PDF app.
 */
function SignWorkspace({ document }: { document: OpenedDoc | null }) {
  const [aiOpen, setAiOpen] = useState(true)
  return (
    <div style={{ display: 'flex', width: '100%', height: '100%' }}>
      <div style={{ flex: 1, minWidth: 0 }}>
        <SignApp
          file={
            document
              ? new File([document.bytes as unknown as BlobPart], document.name, {
                  type: 'application/pdf',
                })
              : null
          }
        />
      </div>
      {aiOpen ? (
        <div
          style={{
            width: 360,
            flexShrink: 0,
            borderLeft: '1px solid var(--ui-border-default)',
            background: 'var(--surface-panel, #fff)',
          }}
        >
          <OfficeAiSlot
            appKey="pdf"
            close={() => setAiOpen(false)}
            fallback={
              <div
                style={{
                  display: 'flex',
                  flexDirection: 'column',
                  alignItems: 'center',
                  justifyContent: 'center',
                  gap: 8,
                  height: '100%',
                  padding: 24,
                  textAlign: 'center',
                  color: 'var(--ui-text-secondary)',
                  fontSize: 'var(--text-sm)',
                }}
              >
                <span>No built-in assistant in Sign.</span>
                <span>Switch to the Allternit Assistant tab to chat about the open PDF.</span>
              </div>
            }
          />
        </div>
      ) : (
        <button
          type="button"
          className="btn btn-ghost"
          style={{
            width: 34,
            flexShrink: 0,
            writingMode: 'vertical-rl',
            borderRadius: 0,
            borderLeft: '1px solid var(--ui-border-default)',
          }}
          title="Open AI assistant"
          onClick={() => setAiOpen(true)}
        >
          AI
        </button>
      )}
    </div>
  )
}

function CloudPromptBanner() {
  return (
    <div
      style={{
        flexShrink: 0,
        padding: 'var(--space-2) var(--space-3)',
        background: 'color-mix(in srgb, var(--accent-primary) 12%, transparent)',
        borderBottom: '1px solid var(--ui-border-default)',
        fontSize: 'var(--text-sm)',
        color: 'var(--ui-text-primary)',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'space-between',
        gap: 'var(--space-3)',
      }}
    >
      <span>
        Running locally without sign-in.{' '}
        <span style={{ color: 'var(--ui-text-secondary)' }}>
          Sign in to Allternit to use the AI assistant and cloud features.
        </span>
      </span>
      <SignInButton mode="modal">
        <button type="button" className="btn btn-primary">
          Sign in to Allternit
        </button>
      </SignInButton>
    </div>
  )
}

function OfficeWorkspace({
  onBack,
  initialTab = 'docs',
}: {
  onBack: () => void
  initialTab?: AppTab
}) {
  const { isSignedIn, getToken } = useAuth()
  const [activeTab, setActiveTab] = useState<AppTab>(initialTab)
  const [documents, setDocuments] = useState<Partial<Record<AppTab, OpenedDoc>>>({})
  const [mountKey, setMountKey] = useState(0)

  useEffect(() => {
    // /api/agent-chat verifies this token; signed out, no token is sent.
    setOfficeAiTokenProvider(() => (isSignedIn ? getToken() : null))
    return () => setOfficeAiTokenProvider(null)
  }, [isSignedIn, getToken])

  const host = useMemo<OfficeHost>(() => {
    const ai = createStandaloneAiClient({ getIsSignedIn: () => isSignedIn ?? false })
    return createBrowserHost({
      getLanguage: () => 'en',
      ai,
      extensions: [createAllternitAssistantExtension()],
    })
  }, [isSignedIn])

  const handleOpenFile = useCallback(async () => {
    const result = await host.openFile({ accept: ACCEPT_MAP[activeTab], multiple: false })
    if (!result) return
    const opened = Array.isArray(result) ? result[0] : result
    if (!opened) return
    setDocuments((prev) => ({ ...prev, [activeTab]: { name: opened.name, bytes: opened.bytes } }))
    setMountKey((k) => k + 1)
  }, [host, activeTab])

  const documentForTab = documents[activeTab]

  return (
    <OfficeHostProvider host={host}>
      <div
        style={{
          width: '100%',
          height: '100%',
          display: 'flex',
          flexDirection: 'column',
        }}
      >
        <nav
          style={{
            flexShrink: 0,
            height: 48,
            display: 'flex',
            alignItems: 'center',
            gap: 'var(--space-1)',
            padding: '0 var(--space-3)',
            borderBottom: '1px solid var(--ui-border-default)',
            background: 'var(--surface-panel)',
          }}
        >
          <button
            type="button"
            onClick={onBack}
            className="btn btn-ghost"
            style={{
              color: 'var(--accent-primary)',
              marginRight: 'var(--space-3)',
              fontWeight: 'var(--font-weight-semibold)',
            }}
          >
            Allternit Office
          </button>
          {TABS.map((tab) => {
            const isActive = activeTab === tab.id
            return (
              <button
                key={tab.id}
                type="button"
                onClick={() => setActiveTab(tab.id)}
                className={isActive ? 'btn btn-primary' : 'btn btn-ghost'}
                style={{
                  padding: 'var(--space-1) var(--space-3)',
                  borderRadius: 'var(--radius-sm)',
                }}
              >
                {tab.label}
              </button>
            )
          })}
          <div style={{ flex: 1 }} />
          <button
            type="button"
            onClick={handleOpenFile}
            className="btn btn-secondary"
            style={{ padding: 'var(--space-1) var(--space-3)' }}
          >
            Open file
          </button>
        </nav>

        <CloudPromptBanner />

        <main style={{ flex: 1, minHeight: 0, position: 'relative' }}>
          {/* Keep every app mounted but hidden so heavy editors do not unmount/remount on tab switches. */}
          {/* Changing key remounts the active app so the new document is loaded. */}
          <div
            style={{
              display: activeTab === 'docs' ? 'block' : 'none',
              width: '100%',
              height: '100%',
            }}
            key={activeTab === 'docs' ? `docs-${mountKey}` : 'docs'}
          >
            <DocsApp language="en" document={documentForTab ?? null} />
          </div>
          <div
            style={{
              display: activeTab === 'sheets' ? 'block' : 'none',
              width: '100%',
              height: '100%',
            }}
            key={activeTab === 'sheets' ? `sheets-${mountKey}` : 'sheets'}
          >
            <SheetsApp language="en" document={documentForTab ?? null} />
          </div>
          <div
            style={{
              display: activeTab === 'slides' ? 'block' : 'none',
              width: '100%',
              height: '100%',
            }}
            key={activeTab === 'slides' ? `slides-${mountKey}` : 'slides'}
          >
            <SlidesApp language="en" document={documentForTab ?? null} />
          </div>
          <div
            style={{
              display: activeTab === 'pdf' ? 'block' : 'none',
              width: '100%',
              height: '100%',
            }}
            key={activeTab === 'pdf' ? `pdf-${mountKey}` : 'pdf'}
          >
            <PdfApp language="en" document={documentForTab ?? null} />
          </div>
          <div
            style={{
              display: activeTab === 'sign' ? 'block' : 'none',
              width: '100%',
              height: '100%',
            }}
            key={activeTab === 'sign' ? `sign-${mountKey}` : 'sign'}
          >
            <SignWorkspace document={documentForTab ?? null} />
          </div>
        </main>
      </div>
    </OfficeHostProvider>
  )
}

function AppContent() {
  const [showOffice, setShowOffice] = useState(false)
  const [initialTab, setInitialTab] = useState<AppTab>('docs')

  const handleLaunch = (tab?: AppTab) => {
    if (tab) setInitialTab(tab)
    setShowOffice(true)
  }

  const handleBack = () => {
    setShowOffice(false)
    setInitialTab('docs')
  }

  if (!showOffice) {
    return (
      <HomePage
        onLaunch={handleLaunch}
        disclosure={
          <span>
            Allternit Office works locally in your browser with no account required.{' '}
            <strong>Sign in to Allternit to unlock the AI assistant and cloud sync.</strong>
          </span>
        }
      />
    )
  }

  return <OfficeWorkspace onBack={handleBack} initialTab={initialTab} />
}

export function App() {
  return (
    <ClerkProvider publishableKey={CLERK_PUBLISHABLE_KEY}>
      <AppContent />
    </ClerkProvider>
  )
}
