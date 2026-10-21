# Allternit Bonsai Local Companion

This companion runs PrismML Bonsai Image 4B entirely on an Apple Silicon Mac and exposes only a loopback HTTP endpoint to Allternit.

The installer pins auditable inputs:

- PrismML Image Studio: `31b02171634c16b5da0eec6aea075e7489d5fb39` (Apache-2.0)
- Bonsai ternary model: `2c24c81b934a658ba5590cf39088ba929985b4a8`
- MLX runtime: official PyPI arm64 wheel `mlx==0.31.1`

Run `./install.sh`, then `./start.sh`. Installation data lives under `~/Library/Application Support/Allternit/bonsai-local` and can be removed without touching user documents. The server binds to `127.0.0.1:8000`; browser access is restricted to local Allternit development and production origins.

The companion requires Apple Silicon, Python 3.13 (managed by `uv`), `uv`, and at least 8 GiB of free storage. Full Xcode is **not** required: upstream's pinned MLX git fork differs from upstream by one guard in the 1-bit Metal fast path, which the 2-bit ternary model never exercises, so the installer substitutes the official prebuilt `mlx` wheel. This path was verified end-to-end on 2026-07-15 (seed-42 generation produced byte-identical PNGs across runs).
