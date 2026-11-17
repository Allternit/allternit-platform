# allternit-mirror

The `mirror` Phone Use route on a Mac
(`docs/contracts/CONTRACT-phone-use-2026-11-16.md`, route 2a). Apple's iPhone
Mirroring app shows the user's iPhone in a window. With the user's permission,
Allternit Desktop sees that window and clicks and types inside it, the way the
user would with their own mouse and keyboard.

Desktop runs this tool from `Contents/Resources/bin/allternit-mirror` and talks
to it in `surfaces/allternit-desktop/src/main/phone-mirror.ts`. It speaks
newline-delimited JSON on stdin and stdout. The protocol is documented in
`Sources/MirrorCore/Protocol.swift`. Keep both sides in sync.

## What it does

| Command | Does |
|---|---|
| `probe` | Checks whether the app is installed and running, finds its window, reads the Mac's lock state and the Screen Recording and Accessibility permissions, and with `ocr` reads the window for "iPhone in Use". It returns one health state. |
| `capture` | Takes a PNG of that one window (no cursor, no shadow) and returns its pixel size. With `gray`, it also returns a small grayscale copy for Desktop's changed-screen check. |
| `read` | Runs Vision text recognition and returns `{class:"Text", label, id:"", bounds:[x1,y1,x2,y2]}` lines in capture pixels. This is the `phone.read_ui` element shape. |
| `tap`, `swipe`, `back` | Mouse events at points inside the window. |
| `type`, `key`, `home`, `app_switcher`, `spotlight` | Key events sent to the Mirroring process only. |

### Keeping input inside the window

- The window is iPhone Mirroring's only visible, normal-layer window. The tool
  refuses when no window is showing or when more than one is.
- Coordinates are pixels of the last capture. The tool refuses a point outside
  `[0, width) × [0, height)`. The converted screen point must also lie inside
  the window's frame.
- Before a click or drag, the tool brings the window forward. It then waits up
  to 1 s until no other app's window covers the target points, and refuses if
  one still does. A swipe checks its start, middle and end points.
- Keys are posted to the Mirroring process with `CGEvent.postToPid`, so they
  cannot land in another app.
- After a click or drag, the pointer returns to where the user left it.

### Limits

- Every command has a time limit. Past it, the tool answers `timeout` and
  exits, and Desktop starts a new process. The tool also exits when stdin
  closes.
- It never asks for or grants a permission. It only checks them with the
  preflight calls. When one is missing, it names the System Settings pane.
  macOS gives the permission to Allternit Desktop, the app that starts this
  tool.

## Not checked on hardware (plan check P1)

These points are unverified until the owner's device check:

- whether iPhone Mirroring accepts synthetic mouse events and keys posted to a
  process;
- whether Cmd+1, Cmd+2 and Cmd+3 open Home, the App Switcher and Spotlight;
- the wording of the "iPhone in Use" screen.

## Build

```sh
swift build -c release --arch arm64 --arch x86_64 --package-path cmd/allternit-mirror
swift test --package-path cmd/allternit-mirror
```

The tool needs macOS 15 or later and uses public frameworks only:
ScreenCaptureKit, Vision, CoreGraphics/ApplicationServices and AppKit.
Desktop packaging stages the binary with
`surfaces/allternit-desktop/scripts/prepare-mirror.cjs`. The release workflow
builds it in `release-desktop.yml`. `.github/workflows/allternit-mirror.yml` is
the standalone check.

## Licence

All code here is Allternit's own. mirroir-mcp (Apache-2.0) was a reference for
its public description only. No code was copied from it or from any other
project, so no NOTICE entry is needed.
