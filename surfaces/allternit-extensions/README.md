# Allternit Extensions

Browser extensions, Office add-ins, and the native Office Suite integration that bring the Allternit AI assistant to every surface.

## Extensions

### Native Office Suite panel (Allternit Assistant)

The Allternit Office Suite (`@allternit/office-suite`) has a first-class extension slot: a host registers `extensions` on the `OfficeHost` contract, and the registered extension **occupies the existing AI chat section** inside each office app (Docs, Sheets, Slides, PDF). When no extensions are registered, the app's built-in AI panel renders unchanged.

- **Allternit Assistant** (`platform/packages/allternit-office-suite/src/extensions/`) — chat/agent panel on the suite's `useOfficeAi()` contract: per-app model picker, streaming agent loop, active-document awareness.
- **Mounted in** `surfaces/ai.allternit.com` platform views (`src/views/{docs,sheets,slides,pdf}`) — which is what the desktop app's Office windows load.

### `allternit-extension` — Chrome Extension

The original Allternit extension for Chrome. Injects a side panel into any web page, powered by the `ExtensionSidepanelShell` shared component.

- **Debug mode**: Chrome → Extensions → Load unpacked → select `dist/`
- **Build**: WXT (Web Extension Toolkit)
- **Storage**: `chrome.storage.local`
- **Docs**: See `allternit-extension/README.md`

### `allternit-office-addin` — Microsoft Office Add-in

A task pane add-in for Excel, PowerPoint, and Word. Shares the same UI shell and adapter pattern as the Chrome extension, with Office.js-specific bridges and per-host plugin system.

- **Debug mode**: `npm run dev:excel` / `npm run dev:powerpoint` / `npm run dev:word`
- **Build**: Vite + `office-addin-debugging`
- **Storage**: `OfficeRuntime.storage` (with `localStorage` fallback)
- **Docs**: See `allternit-office-addin/README.md` and `allternit-office-addin/ARCHITECTURE.md`

---

## Shared Architecture

Both extensions use the same canonical shell:

```
surfaces/allternit-extensions/extension-shared/extension-sidepanel/
├── ExtensionSidepanelShell.tsx       ← shared UI shell
└── ExtensionSidepanelShell.types.ts  ← ExtensionSidepanelAdapter interface
```

Each extension implements `ExtensionSidepanelAdapter` to bridge its platform-specific API (Chrome / Office.js) to the shared shell.

```
ExtensionSidepanelAdapter
├── execute(task) → Promise<void>   ← runs the AI task
├── stop() → void                  ← aborts the current task
├── status                         ← idle | running | completed | error
├── history                        ← conversation history
└── config                         ← persisted settings
```

### Design Tokens

Both extensions use the Allternit sand/nude palette:

| Token | Value | Usage |
|---|---|---|
| `--bg-primary` | `#FDF8F3` | Main background |
| `--accent-primary` | `#B08D6E` | Primary accent, buttons |
| `--text-primary` | `#2A1F16` | Body text |
| `--bg-secondary` | `#F5EDE3` | Card backgrounds |

Source of truth: `surfaces/ai.allternit.com/src/design/theme.css`

---

## Comparison

| Feature | Chrome Extension | Office Add-in |
|---|---|---|
| Debug/dev mode | Load unpacked | `office-addin-debugging` |
| Storage | `chrome.storage.local` | `OfficeRuntime.storage` |
| Host detection | `window.location` / tab URL | `Office.context.host` |
| Code execution | N/A | `code-executor.ts` (Office.js via `new Function`) |
| Plugin system | No | Yes — per-host (Excel/PPT/Word) |
| Streaming | `fetch` SSE | `fetch` SSE (same pattern) |
| Build tool | WXT | Vite |
| Bundle target | Chrome MV3 | Single HTML task pane |
