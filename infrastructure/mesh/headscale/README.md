# Headscale control plane

> **STATUS: LEGACY (2026-10-17).** This Fly.io app (`allternit-headscale`,
> region lax) is no longer the production control plane. Production Headscale
> runs on the mail VPS at `https://mail.news.allternit.com:8444` with the gRPC
> admin socket local to that machine (`/etc/headscale/config.yaml` there is
> the live config). The platform enrollment route
> (`POST /api/v1/mesh/enroll` in `cmd/allternit-cloud-api`) is wired against
> the VPS instance. This directory is kept as the reference deploy recipe and
> policy source; the checked-in `config.yaml` has been brought in line with
> production (embedded DERP region 999, no Tailscale-run relays), but nothing
> here is deployed automatically — do not `fly deploy` without an explicit
> decision to revive this app.

Self-hosted Tailscale coordination server for the Allternit mesh (gizzi-code
desktop/VPS nodes + tsnet-embedded iOS/Android apps), deployed as the Fly.io
app `allternit-headscale` (region lax, SQLite on a Fly volume).

Everything operational — deploy, user/preauth-key management, node admin,
backups, upgrades — is in [OPS.md](./OPS.md).
