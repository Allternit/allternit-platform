# Vendored: allternitos-cloud-contracts

This crate is **vendored** from the private `Gizziio/AllternitOS` monorepo so
that this repository (and CI) can build without a second checkout.

- **Source:** `fabric/os/cloud-contracts` in https://github.com/Gizziio/AllternitOS
- **Upstream rev:** `1a7be05` (2026-09-03)
- **Why vendored:** the previous root `Cargo.toml` path dep
  (`../../AllternitOS/fabric/os/cloud-contracts`) escaped the repo, which
  broke `cargo metadata`/`cargo build` in CI
  (`deploy-cloud-api-contabo.yml`, 5 consecutive failures). A `git =` dep was
  rejected because the AllternitOS repo is private and CI has no credentials
  for it.

## Updating

Authority flipped 2026-11-27: this platform repo is now the source of truth
for the os contracts. The new contract package is `spec/Contracts/os/v1`
(sibling package, PR in flight). This crate is edited here, and the old
`Gizziio/AllternitOS` repo is archived pending Eoj's decision; do not copy
changes from it. The "Upstream rev" above is historical only.
