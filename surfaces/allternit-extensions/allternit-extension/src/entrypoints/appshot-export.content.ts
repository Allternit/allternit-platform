/**
 * Appshot Export — Content Script
 *
 * Injects a floating "Capture Appshot" button on supported pages and handles
 * capture requests from the sidepanel. Communicates via chrome.runtime
 * messaging and chrome.tabs messaging.
 *
 * Supported pages: all http/https URLs (excludes chrome://, about:, etc.)
 */

import { captureFullAppshot } from '@/lib/appshots/capture'
import { saveAppshot } from '@/lib/appshots/storage'
import type { Appshot } from '@/lib/appshots/types'

const DEBUG_PREFIX = '[Appshot Export]'

export default defineContentScript({
	matches: ['http://*/*', 'https://*/*'],
	runAt: 'document_idle',

	main() {
		console.debug(`${DEBUG_PREFIX} Loaded on ${window.location.href}`)

		// Don't inject on chrome-extension or about pages
		if (
			window.location.protocol === 'chrome-extension:' ||
			window.location.protocol === 'about:' ||
			window.location.protocol === 'chrome:'
		) {
			return
		}

		// Inject the floating capture button
		injectCaptureButton()

		// Listen for capture requests from the sidepanel / background
		chrome.runtime.onMessage.addListener((message, _sender, sendResponse): true | undefined => {
			if (message.type === 'APPSHOT_CAPTURE_PAGE') {
				handleCaptureRequest(message.title)
					.then((appshot) => sendResponse({ appshot }))
					.catch((err) => {
						console.error(`${DEBUG_PREFIX} Capture failed`, err)
						sendResponse({ error: (err as Error).message })
					})
				return true // async sendResponse
			}
			return undefined
		})
	},
})

// ── Floating capture button ───────────────────────────────────────────────────

function injectCaptureButton(): void {
	// Avoid double-injection
	if (document.querySelector('.allternit-appshot-fab')) return

	const fab = document.createElement('button')
	fab.className = 'allternit-appshot-fab'
	fab.title = 'Capture Appshot'
	fab.setAttribute('aria-label', 'Capture Appshot of this page')

	// Position: bottom-right corner, above any existing UI
	Object.assign(fab.style, {
		position: 'fixed',
		bottom: '24px',
		right: '24px',
		zIndex: '2147483646', // just below max
		width: '48px',
		height: '48px',
		borderRadius: '50%',
		border: 'none',
		cursor: 'pointer',
		display: 'flex',
		alignItems: 'center',
		justifyContent: 'center',
		background: '#1F1E1D',
		color: '#FDF8F3',
		boxShadow: '0 4px 12px rgba(0, 0, 0, 0.25)',
		transition: 'transform 0.2s ease, box-shadow 0.2s ease, opacity 0.3s ease',
		fontFamily: 'system-ui, -apple-system, sans-serif',
		fontSize: '20px',
		lineHeight: '1',
		opacity: '0.7',
		padding: '0',
		outline: 'none',
	} as Partial<CSSStyleDeclaration>)

	// Camera / snapshot icon (SVG inline)
	fab.innerHTML = `
		<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
			<path d="M23 19a2 2 0 0 1-2 2H3a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h4l2-3h6l2 3h4a2 2 0 0 1 2 2z"/>
			<circle cx="12" cy="13" r="4"/>
		</svg>
	`

	// Hover effects
	fab.addEventListener('mouseenter', () => {
		fab.style.transform = 'scale(1.1)'
		fab.style.boxShadow = '0 6px 20px rgba(0, 0, 0, 0.32)'
		fab.style.opacity = '1'
	})

	fab.addEventListener('mouseleave', () => {
		fab.style.transform = 'scale(1)'
		fab.style.boxShadow = '0 4px 12px rgba(0, 0, 0, 0.25)'
		fab.style.opacity = '0.7'
	})

	// Drag support — allow repositioning
	let isDragging = false
	let dragStartTime = 0
	let startX = 0
	let startY = 0
	let startRight = 0
	let startBottom = 0

	fab.addEventListener('mousedown', (e) => {
		isDragging = false
		dragStartTime = Date.now()
		startX = e.clientX
		startY = e.clientY
		const rect = fab.getBoundingClientRect()
		startRight = window.innerWidth - rect.right
		startBottom = window.innerHeight - rect.bottom
	})

	fab.addEventListener('mousemove', (e) => {
		if (e.buttons !== 1) return
		const dx = e.clientX - startX
		const dy = e.clientY - startY
		if (Math.abs(dx) > 3 || Math.abs(dy) > 3) {
			isDragging = true
			fab.style.right = `${Math.max(0, startRight - dx)}px`
			fab.style.bottom = `${Math.max(0, startBottom - dy)}px`
		}
	})

	fab.addEventListener('mouseup', () => {
		const wasDrag = isDragging || Date.now() - dragStartTime > 300
		isDragging = false
		if (wasDrag) return // was a drag, not a click
	})

	fab.addEventListener('click', (e) => {
		// If it was a drag gesture, suppress the click
		if (isDragging || Date.now() - dragStartTime > 300) {
			e.preventDefault()
			e.stopPropagation()
			return
		}
		handleFabClick(fab)
	})

	document.documentElement.appendChild(fab)
}

// ── Fab click handler ─────────────────────────────────────────────────────────

async function handleFabClick(fab: HTMLButtonElement): Promise<void> {
	// Visual feedback: scale down + change icon to spinner
	fab.style.transform = 'scale(0.9)'
	fab.style.opacity = '1'
	const originalHTML = fab.innerHTML
	fab.innerHTML = `
		<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
			style="animation: allternit-appshot-spin 0.8s linear infinite;">
			<path d="M21 12a9 9 0 1 1-6.219-8.56"/>
		</svg>
	`

	// Inject spinner keyframes if not already present
	if (!document.getElementById('allternit-appshot-spinner-style')) {
		const style = document.createElement('style')
		style.id = 'allternit-appshot-spinner-style'
		style.textContent = `@keyframes allternit-appshot-spin { to { transform: rotate(360deg); } }`
		document.head.appendChild(style)
	}

	try {
		const appshot = await handleCaptureRequest(undefined)
		showToast(fab, `Appshot saved: ${appshot.title}`)
	} catch (err) {
		console.error(`${DEBUG_PREFIX} Capture failed`, err)
		showToast(fab, `Capture failed: ${(err as Error).message}`, true)
	} finally {
		fab.innerHTML = originalHTML
		fab.style.transform = 'scale(1)'
		fab.style.opacity = '0.7'
	}
}

// ── Capture orchestration ─────────────────────────────────────────────────────

async function handleCaptureRequest(title?: string): Promise<Appshot> {
	console.debug(`${DEBUG_PREFIX} Starting capture…`)

	const appshot = await captureFullAppshot(title)

	// Persist locally
	await saveAppshot(appshot)

	console.debug(`${DEBUG_PREFIX} Captured appshot: ${appshot.id}`)

	// Notify background (optional — for analytics or cross-tab features)
	try {
		await chrome.runtime.sendMessage({
			type: 'APPSHOT_CAPTURE_RESULT',
			appshot,
		})
	} catch {
		// Background may not be listening — safe to ignore
	}

	return appshot
}

// ── Toast notification ────────────────────────────────────────────────────────

function showToast(fab: HTMLElement, message: string, isError = false): void {
	const toast = document.createElement('div')
	toast.className = 'allternit-appshot-toast'

	Object.assign(toast.style, {
		position: 'fixed',
		bottom: '80px',
		right: '24px',
		zIndex: '2147483647',
		padding: '8px 16px',
		borderRadius: '8px',
		background: isError ? '#FEE2E2' : '#F5EDE3',
		color: isError ? '#991B1B' : '#2A1F16',
		fontFamily: 'system-ui, -apple-system, sans-serif',
		fontSize: '12px',
		lineHeight: '1.4',
		boxShadow: '0 4px 12px rgba(0,0,0,0.15)',
		maxWidth: '280px',
		opacity: '0',
		transform: 'translateY(8px)',
		transition: 'opacity 0.3s ease, transform 0.3s ease',
		pointerEvents: 'none',
		border: `1px solid ${isError ? '#FECACA' : '#D9C9B8'}`,
	} as Partial<CSSStyleDeclaration>)

	toast.textContent = message
	document.documentElement.appendChild(toast)

	// Animate in
	requestAnimationFrame(() => {
		toast.style.opacity = '1'
		toast.style.transform = 'translateY(0)'
	})

	// Animate out and remove
	setTimeout(() => {
		toast.style.opacity = '0'
		toast.style.transform = 'translateY(8px)'
		setTimeout(() => toast.remove(), 300)
	}, 3000)
}
