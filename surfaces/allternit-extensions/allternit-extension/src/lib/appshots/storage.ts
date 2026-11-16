/**
 * Appshots — storage layer using chrome.storage.local.
 *
 * Provides CRUD operations plus export/share helpers for persisting
 * Appshot records within the Chrome extension's local storage quota.
 */

import type { Appshot } from './types'

const STORAGE_PREFIX = 'appshot:'
const INDEX_KEY = 'appshot:__index'

// ── Internal helpers ──────────────────────────────────────────────────────────

/** Build the storage key for a single appshot. */
function key(id: string): string {
	return `${STORAGE_PREFIX}${id}`
}

/** Read the ordered index of appshot IDs (newest-first). */
async function readIndex(): Promise<string[]> {
	const result = await chrome.storage.local.get(INDEX_KEY)
	return Array.isArray(result[INDEX_KEY]) ? result[INDEX_KEY] : []
}

/** Persist the index array. */
async function writeIndex(ids: string[]): Promise<void> {
	await chrome.storage.local.set({ [INDEX_KEY]: ids })
}

// ── Public API ────────────────────────────────────────────────────────────────

/** Persist an appshot. Updates the index if the ID already exists. */
export async function saveAppshot(appshot: Appshot): Promise<void> {
	const ids = await readIndex()

	// Remove existing entry (if updating) then prepend
	const next = [appshot.id, ...ids.filter((id) => id !== appshot.id)]

	await chrome.storage.local.set({
		[key(appshot.id)]: appshot,
		[INDEX_KEY]: next,
	})
}

/** Retrieve all saved appshots, newest first. */
export async function listAppshots(): Promise<Appshot[]> {
	const ids = await readIndex()
	if (ids.length === 0) return []

	const keys = ids.map(key)
	const raw = await chrome.storage.local.get(keys)

	return ids
		.map((id) => raw[key(id)] as Appshot | undefined)
		.filter((a): a is Appshot => a !== undefined)
}

/** Get a single appshot by ID. */
export async function getAppshot(id: string): Promise<Appshot | undefined> {
	const raw = await chrome.storage.local.get(key(id))
	return raw[key(id)] as Appshot | undefined
}

/** Remove an appshot from storage. */
export async function deleteAppshot(id: string): Promise<void> {
	const ids = await readIndex()
	await chrome.storage.local.remove(key(id))
	await writeIndex(ids.filter((i) => i !== id))
}

/** Remove all appshots from storage. */
export async function clearAppshots(): Promise<void> {
	const ids = await readIndex()
	const keys = ids.map(key)
	await chrome.storage.local.remove([...keys, INDEX_KEY])
}

// ── Export helpers ─────────────────────────────────────────────────────────────

/**
 * Generate a self-contained HTML file that replays the appshot.
 * Returns the HTML string — the caller can trigger a download or save to disk.
 */
export function exportAppshot(appshot: Appshot): string {
	const messages = appshot.agentContext?.messages ?? []
	const toolCalls = appshot.agentContext?.toolCalls ?? []

	const messagesHtml = messages
		.map(
			(m) => `
		<div class="message message-${m.role}">
			<div class="role">${m.role}</div>
			<div class="content">${escapeHtml(m.content)}</div>
			<div class="timestamp">${m.timestamp}</div>
		</div>`,
		)
		.join('\n')

	const toolCallsHtml = toolCalls
		.map(
			(tc) => `
		<div class="tool-call">
			<div class="tool-name">${escapeHtml(tc.name)}</div>
			<pre class="tool-input">${escapeHtml(JSON.stringify(tc.input, null, 2))}</pre>
			${tc.output ? `<pre class="tool-output">${escapeHtml(tc.output)}</pre>` : ''}
			<div class="timestamp">${tc.timestamp}</div>
		</div>`,
		)
		.join('\n')

	return `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8" />
<meta name="viewport" content="width=device-width, initial-scale=1.0" />
<title>${escapeHtml(appshot.title)} — Appshot</title>
<style>
	:root {
		--bg-primary: #FDF8F3;
		--bg-secondary: #F5EDE3;
		--accent-primary: #1F1E1D;
		--text-primary: #2A1F16;
		--text-secondary: #6B5B4E;
		--border: #D9C9B8;
	}
	* { margin: 0; padding: 0; box-sizing: border-box; }
	body {
		font-family: 'Inter', system-ui, -apple-system, sans-serif;
		background: var(--bg-primary);
		color: var(--text-primary);
		line-height: 1.6;
		padding: 2rem;
		max-width: 900px;
		margin: 0 auto;
	}
	header {
		border-bottom: 2px solid var(--accent-primary);
		padding-bottom: 1rem;
		margin-bottom: 2rem;
	}
	header h1 { font-size: 1.5rem; font-weight: 700; }
	header .meta {
		font-size: 0.85rem;
		color: var(--text-secondary);
		margin-top: 0.25rem;
	}
	header .meta a { color: var(--accent-primary); text-decoration: none; }
	section { margin-bottom: 2rem; }
	section h2 {
		font-size: 1.1rem;
		font-weight: 600;
		margin-bottom: 0.75rem;
		color: var(--accent-primary);
	}
	.message {
		background: var(--bg-secondary);
		border-radius: 8px;
		padding: 0.75rem 1rem;
		margin-bottom: 0.5rem;
		border-left: 3px solid transparent;
	}
	.message-user { border-left-color: var(--accent-primary); }
	.message-assistant { border-left-color: #5B8C6F; }
	.message .role {
		font-size: 0.75rem;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		color: var(--text-secondary);
	}
	.message .content { margin-top: 0.25rem; white-space: pre-wrap; }
	.message .timestamp { font-size: 0.7rem; color: var(--text-secondary); margin-top: 0.25rem; }
	.tool-call {
		background: var(--bg-secondary);
		border-radius: 8px;
		padding: 0.75rem 1rem;
		margin-bottom: 0.5rem;
	}
	.tool-call .tool-name {
		font-family: 'JetBrains Mono', monospace;
		font-size: 0.85rem;
		font-weight: 600;
		color: var(--accent-primary);
	}
	.tool-call pre {
		font-family: 'JetBrains Mono', monospace;
		font-size: 0.75rem;
		background: var(--bg-primary);
		border: 1px solid var(--border);
		border-radius: 4px;
		padding: 0.5rem;
		margin-top: 0.25rem;
		overflow-x: auto;
	}
	.tool-call .timestamp { font-size: 0.7rem; color: var(--text-secondary); margin-top: 0.25rem; }
	.snapshot-frame {
		border: 1px solid var(--border);
		border-radius: 8px;
		overflow: hidden;
		background: white;
	}
	.snapshot-frame iframe {
		width: 100%;
		min-height: 400px;
		border: none;
	}
	footer {
		text-align: center;
		font-size: 0.75rem;
		color: var(--text-secondary);
		margin-top: 3rem;
		padding-top: 1rem;
		border-top: 1px solid var(--border);
	}
</style>
</head>
<body>
	<header>
		<h1>${escapeHtml(appshot.title)}</h1>
		<div class="meta">
			Captured ${escapeHtml(appshot.createdAt)}
			&middot; <a href="${escapeHtml(appshot.pageUrl)}" target="_blank" rel="noopener">${escapeHtml(appshot.pageTitle || appshot.pageUrl)}</a>
			${appshot.agentContext ? ` &middot; ${appshot.agentContext.model} &middot; ${appshot.agentContext.totalTokens.toLocaleString()} tokens` : ''}
		</div>
	</header>

	${
		appshot.capturedHtml
			? `<section>
		<h2>Page Snapshot</h2>
		<div class="snapshot-frame">
			<iframe sandbox="allow-same-origin" srcdoc="${escapeAttr(appshot.capturedHtml)}"></iframe>
		</div>
	</section>`
			: ''
	}

	${
		messages.length > 0
			? `<section>
		<h2>Agent Conversation</h2>
		${messagesHtml}
	</section>`
			: ''
	}

	${
		toolCalls.length > 0
			? `<section>
		<h2>Tool Calls</h2>
		${toolCallsHtml}
	</section>`
			: ''
	}

	<footer>
		Generated by Allternit Appshots
	</footer>
</body>
</html>`
}

/**
 * Create a shareable link for the appshot.
 * In production this would upload to the Allternit API and return a short URL.
 * For now it generates a local blob URL via the export HTML.
 */
export function generateShareUrl(appshot: Appshot): string {
	// If the appshot already has a share URL, return it
	if (appshot.shareUrl) return appshot.shareUrl

	// Fallback: encode the ID so the sidepanel can reconstruct the link
	// once the backend upload is wired up
	return `allternit://appshot/${appshot.id}`
}

// ── Utilities ─────────────────────────────────────────────────────────────────

function escapeHtml(text: string): string {
	return text
		.replace(/&/g, '&amp;')
		.replace(/</g, '&lt;')
		.replace(/>/g, '&gt;')
		.replace(/"/g, '&quot;')
		.replace(/'/g, '&#39;')
}

function escapeAttr(text: string): string {
	return escapeHtml(text).replace(/\n/g, '&#10;')
}
