# tsnet-ios — Tailscale tsnet embedded in iOS via gomobile

Feasibility spike proving that `tailscale.com/tsnet` (userspace WireGuard
tailnet node) can be cross-compiled into an iOS-consumable
`Mesh.xcframework` with gomobile, and linked from Swift.

**Result: positive.** tsnet v1.102.0 binds and links cleanly with gomobile
on Go 1.26.5 / Xcode 16.2. No c-archive fallback needed.

## Contents

- `go.mod`, `go.sum` — Go module `allternit/mesh`
- `mesh.go` — the gomobile-bound wrapper around `tsnet.Server`
- `build.sh` — idempotent xcframework build
- `Mesh.xcframework/` — build output (not meant to be committed long-term;
  add to `.gitignore` or publish as an artifact)
- `Harness/` — Swift compile/link smoke test

## Verified version pins

| Component | Version |
|---|---|
| Go | 1.26.5 (Homebrew, darwin/arm64) |
| tailscale.com | v1.102.0 |
| golang.org/x/mobile (gomobile + gobind) | v0.0.0-20260709172247-6129f5bee9d5 |
| Xcode | 16.2 (16C5032a), iOS SDK 18.2 |

One-time setup quirk: newer gomobile requires gobind recorded as a Go tool
dependency in the module. `go.mod` already contains
`tool golang.org/x/mobile/cmd/gobind`; if you recreate the module, run
`go get -tool golang.org/x/mobile/cmd/gobind@latest`.

## Rebuild

```sh
export PATH="$PATH:$HOME/go/bin"
./build.sh
```

Runs `gomobile bind -target=ios -iosversion=17.0 -ldflags=-w -o Mesh.xcframework .`,
removing any previous output first. First build takes a few minutes (large
tailscale dep tree); incremental rebuilds take ~10 s.

## Output

- `ios-arm64` (device): `Mesh` binary 25 MB, arm64
- `ios-arm64_x86_64-simulator`: fat static archive 49 MB, arm64 + x86_64

The framework binary is a **static archive** (`ar`), so it links statically
into the app — no dynamic-framework embedding/signing step for the framework
itself, but the ~25 MB lands in the app binary (before App Store thinning).

## Exported API (package `mesh`, Obj-C prefix `Mesh`)

```go
node := mesh.NewNode("my-iphone")            // MeshNewNode("my-iphone")
err  := node.Start(controlURL, authKey, dataDir)
ip   := node.MeshIP()                        // "100.x.y.z" once up, "" otherwise
body, err := node.Get("http://100.x.y.z/")   // through the tailnet
port, err := node.StartProxy("100.x.y.z", 4096) // loopback proxy → 127.0.0.1:port
err  = node.StopProxy()
err  = node.Close()
```

Generated Obj-C surface (see
`Mesh.xcframework/*/Mesh.framework/Headers/Mesh.objc.h`):

- `MeshNewNode(NSString*) -> MeshNode?`
- `-[MeshNode start:authKey:dataDir:error:] -> BOOL`
- `-[MeshNode meshIP] -> NSString*`
- `-[MeshNode get:error:] -> NSString*`
- `-[MeshNode startProxy:targetPort:ret0_:error:] -> BOOL`
- `-[MeshNode stopProxy:] -> BOOL`
- `-[MeshNode close:] -> BOOL`

Notes:

- `dataDir` must be a persistent writable dir in the app sandbox, e.g.
  `FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
  .appendingPathComponent("mesh").path` — tsnet stores the node key and
  tailscaled state there. Never `os.UserCacheDir` (the harness API makes the
  caller pass it).
- `Start` blocks until the node is online (up to 60 s) — call it off the main
  thread.
- `StartProxy` exists because iOS networking cannot route 100.64.0.0/10:
  URLSession traffic to mesh URLs must enter the tailnet through the Go side.
  The proxy listens on 127.0.0.1 (random free port), forwards to one fixed
  tailnet target, and carries HTTP/WS unchanged (loopback Host headers pass
  the gizzi server's loopback allowlist). One proxy per node — same target
  returns the running port, a different target replaces it.
- A `Node` is single-use after `Close`; create a new one to reconnect.

## Consuming the xcframework in the app

Either works:

1. **SPM binary target** (recommended for reproducibility): host
   `Mesh.xcframework` at a versioned URL + checksum, declare
   `.binaryTarget(name: "Mesh", url: ..., checksum: ...)` (or
   `path:` for local dev).
2. **Direct embed**: drag `Mesh.xcframework` into the Xcode target →
   Frameworks, Libraries, and Embedded Content → "Do Not Embed" (it is
   static).

Then `import Mesh` in Swift.

## Harness (compile/link proof, do not run)

`Harness/main.swift` calls `MeshNewNode("harness")` and `node.meshIP()`.
It was compiled and linked against the simulator SDK with:

```sh
xcrun --sdk iphonesimulator swiftc Harness/main.swift \
  -F Mesh.xcframework/ios-arm64_x86_64-simulator \
  -framework Mesh \
  -target arm64-apple-ios17.0-simulator \
  -o Harness/harness
```

Result: compiles and links, arm64 simulator binary (~21 MB). It is **not
meant to be run** — `Start` would try to join a tailnet with no control
server/auth key configured. It only proves the framework is consumable from
Swift. Runtime proof (actual tailnet join + `Get`) requires a Headscale/
Tailscale control server and is out of scope for this spike.

## Known constraints for the real iOS integration

- **No NetworkExtension needed**: tsnet is userspace (WireGuard-go + tun
  shim over UDP sockets). Traffic only flows while this process is alive;
  this is NOT a system-wide VPN.
- **App vs app extension**: works in the main app. In extensions (share,
  widgets) memory limits are tight — the Go runtime + tailscale adds tens of
  MB; avoid unless measured.
- **Background execution**: iOS suspends the app in the background and the
  node's UDP sockets die with it; the node drops off the tailnet. Reconnect
  in `applicationDidBecomeActive` (keep the same `dataDir` so the node key is
  reused). VoIP/BGTask tricks are unreliable for this; if always-on mesh is
  required, the real Tailscale app's NetworkExtension model is the only
  robust path.
- **Cold start**: `Start` blocks; run it on a background queue and surface
  errors (bad auth key, unreachable control URL) to the UI.
- **Binary size**: +25 MB per arch (stripped with `-ldflags=-w` already).
- **ATS**: `Get` uses plain HTTP inside the tailnet; App Transport Security
  may block `http://` loads from *URLSession*, but `Get` runs in Go's HTTP
  stack, not URLSession, so ATS does not apply to it.
- **Log noise**: `Logf` is silenced in `mesh.go`; wire it to oslog if
  debugging is needed.
