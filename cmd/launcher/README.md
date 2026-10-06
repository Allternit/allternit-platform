# Allternit Platform Launcher

Single-binary launcher that embeds the Rust API server and UI assets and self-extracts on first run.

## Status

Buildable as a standalone crate. It is intentionally **not** part of the root workspace because compiling it requires pre-built embed artifacts that are produced by the embed pipeline.

## Embed Pipeline

The launcher embeds two artifacts at compile time:

1. `embed/allternit-api` — the compiled `allternit-api` release binary.
2. `embed/ui` — the built static UI assets from private `Allternit/allternit-ai`.

### Building the artifacts

From the repository root:

```bash
./cmd/launcher/script/build-embed.sh
```

This script:

- Builds `allternit-api` in release mode (`cargo build --release -p allternit-api`).
- Builds the UI (`pnpm install && pnpm build` in `Allternit/allternit-ai`, via `scripts/hosted-ui.sh`).
- Copies both artifacts into `cmd/launcher/embed/`.

### Building the launcher

After the embed artifacts exist:

```bash
cd cmd/launcher
cargo build --release -p allternit-platform-launcher
```

The resulting binary is at `target/release/allternit-platform-launcher`.

## Why standalone?

`include_bytes!` and `include_dir!` require the embed paths to exist at compile time. Keeping `cmd/launcher` as its own workspace lets the root workspace (`cargo check --workspace`) pass without forcing every developer to build the full UI and API binary first.

## Runtime behavior

On first run the launcher:

1. Extracts the embedded API binary and UI assets to the user's cache directory.
2. Starts the API server on port `3010`.
3. Starts a minimal static-file UI server on port `3456`.
4. Opens the user's browser to the UI.

Press `Ctrl+C` to stop both servers.
