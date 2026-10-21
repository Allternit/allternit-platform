# allternit Platform

> **Enterprise Agentic Operating System**
> Agent runtime, desktop shell, cloud gateway, and services — one pnpm + Cargo workspace.

This repo (`Allternit/allternit-platform`) is the core platform monorepo: the Allternit agent
runtime (gizzi-code), the desktop app, the Rust API gateway, and the supporting services.
The agent workspace UI (`ai.allternit.com`) lives in the private satellite repo
`Allternit/allternit-ai`, not here.

## Quick start

Requires Node 22+ (pnpm 10 via `packageManager` field) and the stable Rust toolchain
(`rust-toolchain.toml`).

```bash
# Install Node dependencies (pnpm only — never npm/bun for workspace deps)
pnpm install

# Build the main Rust API server
cargo build -p allternit-api

# Run it in dev mode (binds :18013 by default; :8013 only with ALLTERNIT_API_PORT=8013)
make api              # or: pnpm dev:api

# Full dev stack (API + runtime + desktop, via concurrently)
pnpm dev

# Gizzi Code CLI (TUI) dev loop
pnpm tui              # or: bun run --cwd cmd/gizzi-code dev
```

Common checks: `pnpm test` (vitest), `pnpm typecheck`, `make test` (cargo workspace tests).

## Where things live

Ownership rules (source of truth: [REPO_STRUCTURE.md](./REPO_STRUCTURE.md)):

- **`cmd/`** — executables: CLI binaries, API servers, daemons (incl. `gizzi-code`)
- **`services/`** — things that run: long-running services (memory, voice, registry, …)
- **`platform/`** — contracts, protocols, types, plugins, internal `@allternit/*` TS libraries
- **`sdk/`** — public SDK surface only (location frozen)
- **`domains/`** — domain logic (agent, computer-use, governance, kernel)
- **`infrastructure/`** — cloud providers, executors, bridges
- **`surfaces/`** — web apps and desktop surfaces (desktop, mobile, extensions, console, docs)

Legacy consolidations completed 2026-09-18: `packages/@allternit/` → `platform/packages/` (S3); `api/` → `services/` (S2).

Three SDKs: public TS SDK in `sdk/` (tag `sdk/v*`), Rust SDK in `platform/rust-sdk/rust/`
(was `platform/sdk/`; renamed 2026-09-18 so `sdk/` is unambiguous), gizzi SDK in
`cmd/gizzi-code/packages/sdk/` (tag `gizzi-sdk/v*`).

`agent-ledger/` is the signed session record — it stays at the repo root by design.

## Documentation

- [REPO_STRUCTURE.md](./REPO_STRUCTURE.md) — monorepo layout, ownership rules, satellite repos
- [AGENTS.md](./AGENTS.md) — session process: worktrees, PR discipline, ledger attestations
- [docs/](./docs/) — documentation hub (archive, gap-analysis, learnings, reports, specs)
- [DESIGN.md](./docs/design/DESIGN.md) — design system (tokens, typography, colors, animation)
- [docs/MASTER_INDEX.md](./docs/MASTER_INDEX.md) — full documentation index
- [docs/gateway/README.md](./docs/gateway/README.md) — Agent Gateway (AAI): overview, quickstart, adapters, operations
- [docs/public/api/reference.md](./docs/public/api/reference.md) — public API reference

## Service ports

| Service | Port | Runtime | Description |
|---------|------|---------|-------------|
| allternit-cloud-api | 8082 | Rust | Public control plane (`https://api.allternit.com`) — pairing, relay, billing |
| allternit-api | 8013 / 18013 | Rust | Private data-plane runtime (SQLite); dev default 18013 so a stray `cargo run` can't kill the installed gateway |
| gizzi-code runtime | 4096 | Bun / TypeScript | Local agent runtime and LLM session bus |
| platform console | 3013 | Vite / React | `platform.allternit.com` cloud console surface |

## Contributing

See [AGENTS.md](./AGENTS.md). Work happens in your own linked git worktree, lands via PR
(merge commit) on `main`, and ends with a ledger attestation in `agent-ledger/`.

## License

See [LICENSE](./LICENSE) and [NOTICE](./NOTICE).
