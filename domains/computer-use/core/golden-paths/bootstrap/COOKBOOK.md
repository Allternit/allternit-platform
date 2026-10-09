# ACU Bootstrap Cookbook

Standard patterns for connecting Claude (and other models) to different
computer-use surfaces. Each bootstrap returns a ready `ComputerUseExecutor`
that handles the adapter waterfall automatically.

---

## Adapter Waterfall

```
browser.extension  ← PRIMARY: PAGE-AGENT + BROWSER-AGENT via TCP 3011
browser.cdp        ← Chrome/Electron already running with --remote-debugging-port
browser.playwright ← Playwright launches its own headless Chromium
desktop.accessibility ← Native desktop (Quartz events + accessibility tree, macOS)
desktop.terminal   ← Shell command execution
```

Each bootstrap registers the relevant adapter(s). The executor tries
`browser.extension` first and falls down the list automatically.

---

## 1. Browser CDP (Chrome or Chromium)

**When to use:** Automate a real Chrome browser. Works with user-installed Chrome,
Brave, Edge, or any Chromium-based browser.

```python
from golden_paths.bootstrap.browser_cdp_bootstrap import bootstrap_browser_cdp, BrowserCDPConfig

# Auto-detect Chrome, launch if not running, connect on port 9222
executor = await bootstrap_browser_cdp()

# Headed Chrome on a specific port
executor = await bootstrap_browser_cdp(BrowserCDPConfig(port=9222, headless=False))

# Headless Chrome (CI/server)
executor = await bootstrap_browser_cdp(BrowserCDPConfig(headless=True))
```

**What it does:**
1. Checks if `localhost:9222` (or configured port) has a CDP listener
2. If not → launches Chrome with `--remote-debugging-port=9222`
3. Waits up to 20s for Chrome to start
4. Connects `PlaywrightCDPAdapter` and registers it as `browser.cdp`

**Env vars:**
```bash
ACU_CDP_PORT=9222     # CDP port (default: 9222)
ACU_HEADLESS=1        # Force headless mode
```

**Claude system prompt hint:**
> You have access to a real Chrome browser. Use `screenshot` to see the page,
> `navigate` to go to URLs, `left_click` to interact, `extract` to read content.
> Always `screenshot` after each action to confirm the result.

---

## 2. Electron App

**When to use:** Automate an Electron desktop app (VS Code, Slack, Discord,
Figma desktop, etc.). Electron embeds Chromium — use the same CDP path.

```python
from golden_paths.bootstrap.electron_app_bootstrap import bootstrap_electron_app, ElectronAppConfig

# macOS .app bundle — auto-resolves the executable
executor = await bootstrap_electron_app("/Applications/Slack.app")

# Already running on port 9223 (app launched manually with --remote-debugging-port)
executor = await bootstrap_electron_app(cfg=ElectronAppConfig(port=9223))

# Linux/Windows executable
executor = await bootstrap_electron_app("/usr/bin/code")
```

**Launching Electron apps with CDP:**
```bash
# macOS
/Applications/Cursor.app/Contents/MacOS/Cursor --remote-debugging-port=9223

# Linux
code --remote-debugging-port=9223

# Windows
"C:\Program Files\Microsoft VS Code\Code.exe" --remote-debugging-port=9223
```

**What it does:**
1. Resolves the executable (unwraps macOS .app bundles)
2. Checks if CDP port is already reachable
3. If not → launches the Electron app with `--remote-debugging-port`
4. Waits up to 30s for startup
5. Attaches `PlaywrightCDPAdapter` as `browser.cdp`

**Claude system prompt hint:**
> You are controlling [AppName] via its embedded browser engine. Use `screenshot`
> to see the current state. Navigate using `left_click` on UI elements. Use
> `key` for shortcuts (e.g. `ctrl+s` to save). The app is already open.

---

## 3. Desktop Native (macOS)

Native desktop input runs through the `desktop.accessibility` adapter
(Quartz events + the accessibility tree). The gateway registers it at
startup; no separate bootstrap is needed. Grant Accessibility in
*System Settings → Privacy & Security → Accessibility*.

---

## 4. Desktop Terminal

**When to use:** Automate shell tasks, run scripts, interact with CLI tools.

```python
from golden_paths.bootstrap.desktop_terminal_bootstrap import bootstrap_desktop_terminal, DesktopTerminalConfig

# Auto-detect shell (zsh on macOS, bash on Linux, pwsh/cmd on Windows)
executor = await bootstrap_desktop_terminal()

# Specific shell and working directory
executor = await bootstrap_desktop_terminal(
    DesktopTerminalConfig(shell="/bin/bash", cwd="/home/user/project")
)
```

**Running shell commands:**
```python
from core.base_adapter import ActionRequest

result = await executor.execute(
    ActionRequest(action_type="shell", target="ls -la"),
    session_id="my-session",
    run_id="run-001",
)
print(result.extracted_content["stdout"])
print(result.extracted_content["exit_code"])
```

**Supported actions:** `shell` (primary), plus coordinate/keyboard actions
through the accessibility adapter (macOS).

**Claude system prompt hint:**
> You have access to a shell terminal. Use `shell` actions to run commands.
> Always check `exit_code` and `stderr` in the result. Chain commands with
> `&&` for sequential execution. Prefer non-interactive commands.

---

## Combining Adapters (Waterfall)

Register multiple adapters — the executor tries the first, falls back on failure:

```python
from core.computer_use_executor import ComputerUseExecutor
from adapters.browser.extension_adapter import ExtensionAdapter
from golden_paths.bootstrap.browser_cdp_bootstrap import BrowserCDPConfig
from adapters.browser.cdp_adapter import PlaywrightCDPAdapter

executor = ComputerUseExecutor()

# Tier 1: extension (if connected)
try:
    ext = ExtensionAdapter()
    await ext.initialize()
    executor.register("browser.extension", ext)
except Exception:
    pass

# Tier 2: CDP
cdp = PlaywrightCDPAdapter(port=9222)
await cdp.initialize()
executor.register("browser.cdp", cdp)

# Now executor.execute() tries browser.extension first, falls to browser.cdp
```

---

## Quick Reference

| Surface          | Bootstrap function              | Adapter ID          | Requires            |
|------------------|---------------------------------|---------------------|---------------------|
| Browser (Chrome) | `bootstrap_browser_cdp()`       | `browser.cdp`       | Chrome installed    |
| Electron app     | `bootstrap_electron_app(path)`  | `browser.cdp`       | App installed       |
| Desktop native   | (registered by the gateway)     | `desktop.accessibility` | Accessibility perms |
| Shell/terminal   | `bootstrap_desktop_terminal()`  | `desktop.terminal`  | Shell binary        |
| Extension        | `ExtensionAdapter().initialize()`| `browser.extension`| Extension + Desktop app (HTTP 3012)|
