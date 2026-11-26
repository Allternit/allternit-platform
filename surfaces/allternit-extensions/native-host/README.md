# Allternit native host for browser mode/debug extension

This package provides the native messaging bridge used by the Allternit Chrome extension in two modes:

- debug-mode extension in a developer Chrome profile
- native Allternit browser mode / desktop cowork mode

Both modes use the same Chrome native messaging host name:

```text
com.allternit.desktop
```

The extension is a surface for the same computer-use/browser-use protocol used by the browser gateway. It must not be treated as a parallel automation stack.

## Register the host

Chrome native messaging manifests require exact extension origins. The installer always allows both Allternit extension ids (`hkillgnnhkokmjcjjpolpnecmdnilhod`, and the old `chheieepkpbhkiimdmbdjmnhcooclpok`); `--extension-id` / `ALLTERNIT_EXTENSION_ID` add more. Wildcards are not valid for `allowed_origins`.

```bash
ALLTERNIT_EXTENSION_ID=<32-character-chrome-extension-id> pnpm --filter @allternit/desktop-native-host register
```

For browser-mode harnesses that launch with a custom Chromium `--user-data-dir`, install into that profile as well:

```bash
pnpm --filter @allternit/desktop-native-host register -- --extension-id <id> --profile-dir <browser-user-data-dir>
```

For a dry-run/doctor check:

```bash
pnpm --filter @allternit/desktop-native-host run doctor -- --extension-id <32-character-chrome-extension-id>
```

For direct shell usage:

```bash
./register.sh --extension-id <32-character-chrome-extension-id>
./register.sh --doctor --extension-id <32-character-chrome-extension-id>
./register.sh --uninstall
```

## What the installer does

The installer:

1. Creates an executable wrapper at `dist/allternit-native-host-wrapper`.
2. Writes `com.allternit.desktop.json` into supported Chromium native messaging host directories.
3. Sets the manifest `path` to the wrapper executable only.
4. Sets `allowed_origins` to concrete `chrome-extension://<id>/` origins.

This mirrors the shape used by mature browser-debug harnesses: Chrome launches one executable, the wrapper decides whether to run the compiled native host or the development TypeScript source.

## Supported browser manifest locations

On macOS:

- Google Chrome
- Google Chrome for Testing
- Microsoft Edge
- Brave
- Chromium
- Arc

On Linux:

- Google Chrome
- Google Chrome for Testing
- Microsoft Edge
- Brave
- Chromium

Windows writes a common manifest location for registry-based native messaging registration. Registry wiring is intentionally separate from this first deterministic harness pass.

## Protocol path

The native host forwards length-prefixed Chrome native messages to the Allternit Desktop bridge on TCP `127.0.0.1:3011`.

The extension sends/receives protocol messages such as:

```json
{
  "type": "computer_use.action",
  "payload": {
    "surfaceInstanceId": "extension-surface-id",
    "tabId": 123,
    "lease": {},
    "action": {}
  }
}
```

The extension maps that action to its active-tab executor and returns:

```json
{
  "type": "computer_use.events",
  "payload": {
    "events": [],
    "receipt": {}
  }
}
```

## Validation

```bash
pnpm --filter @allternit/desktop-native-host test
pnpm --filter @allternit/desktop-native-host smoke:browser-mode
pnpm --filter @allternit/extension test
pnpm --filter @allternit/extension build
```

`smoke:browser-mode` launches Chrome for Testing with the built development extension, installs the native-host manifest into that temporary browser profile, starts a TCP `3011` bridge, then verifies:

- the deterministic extension service worker starts
- the content script is injected into the page
- `chrome.runtime.connectNative('com.allternit.desktop')` reaches the host and returns `pong`

The smoke stages the native-host binary into the temporary profile before launch. This avoids macOS Desktop-folder execution/TCC failures during local development and matches packaged browser-mode behavior more closely.
