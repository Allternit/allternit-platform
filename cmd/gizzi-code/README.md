# Gizzi Code

AI-powered terminal coding agent for the Allternit ecosystem. Gizzi Code runs
locally, speaks to the LLM provider you configure, and integrates with the
Allternit platform (cloud auth, cowork runs, rails mail, the `alt_` API-key
ecosystem) when you connect it.

## Install

**macOS / Linux:**
```bash
curl -fsSL https://install.gizziio.com/install | bash
```

**Windows (PowerShell):**
```powershell
irm https://install.gizziio.com/install.ps1 | iex
```
PE32+ x64 zip with `checksums.txt` verification. Credentials are stored
via Windows DPAPI (CurrentUser). ARM64 Windows runs the x64 build under
emulation.

**Homebrew (macOS / Linux):**
```bash
brew tap Allternit/tap
brew install gizzi-code
```

**Scoop / winget (Windows):**
```powershell
scoop bucket add allternit https://github.com/Allternit/scoop-bucket
scoop install gizzi-code
winget install Allternit.GizziCode
```

**npm (canonical, all platforms):**
```bash
npm install -g @allternit/gizzi-code
```

Or grab a prebuilt binary from the
[releases page](https://github.com/Allternit/allternit-platform/releases)
(assets `gizzi-code-v<version>-<target>.tar.gz` / `.zip`; tags
`gizzi-code/v<version>`). The repo installer also works without waiting
for install.gizziio.com:

```bash
curl -fsSL https://raw.githubusercontent.com/Allternit/allternit-platform/main/cmd/gizzi-code/install | bash
```

## Platform support

| Platform | Status |
|---|---|
| macOS | Primary target; signed & notarized builds planned |
| Linux | Supported |
| Windows | Supported — PE32+ x64 zip; credentials stored with DPAPI (CurrentUser). ARM64 uses the x64 build under emulation |

## Quick start

```bash
gizzi                    # interactive session
gizzi exec "fix the failing tests"   # one-shot, non-interactive
gizzi serve              # local HTTP/WebSocket server (opt-in)
gizzi api-keys list      # manage Allternit platform tokens
gizzi doctor             # updater + environment health
gizzi --help             # everything else
```

## Configuration

- Provider keys: `ALLTERNIT_API_KEY` for first-party Allternit, plus
  standard provider env vars (`OPENAI_API_KEY`, …) or an auth profile in
  `~/.config/gizzi-code/config.toml`.
- Platform token: `gizzi api-keys set allternit alt_...` (durable scoped keys)
  or run the login flow (`gizzi org status` will point you at it when missing).
- Env overrides: `ALLTERNIT_API_URL`, `GIZZI_PLATFORM_API_URL`,
  `GIZZI_CLERK_ISSUER` — defaults live in `src/shared/constants/cloudUrls.ts`
  and `allternitGateway.ts`.

## Development

```bash
pnpm install                 # from the repo root (pnpm monorepo)
cd cmd/gizzi-code
bun run dev                  # run from source
bun run typecheck            # tsc --noEmit (builds packages/sdk dist first)
bash script/ci-smoke-test.sh # empirically-green test subset
bun run build                # production binary -> dist/gizzi-code
```

## Repository layout

This package lives at `cmd/gizzi-code` inside the
[Allternit/allternit-platform](https://github.com/Allternit/allternit-platform)
monorepo. Platform services it talks to: `cmd/allternit-cloud-api` (public
cloud API, api.allternit.com) and `cmd/allternit-api` (local gateway backend,
loopback-only today — see `reports/2026-09-04-backend-b-deploy-decision.md`).

## Security

See [SECURITY.md](./SECURITY.md). Server mode is opt-in and unauthenticated
without `GIZZI_SERVER_PASSWORD`; the permission system is a UX feature, not a
sandbox.

## Telemetry

Usage telemetry is on by default, anonymous, and never includes prompts,
file contents, or credentials. Disable it with `gizzi config telemetry off`
or `GIZZI_TELEMETRY=off`. Full field-level inventory: [docs/telemetry.md](./docs/telemetry.md).

## License

Apache License 2.0. See [LICENSE](./LICENSE) and [NOTICE](./NOTICE).
