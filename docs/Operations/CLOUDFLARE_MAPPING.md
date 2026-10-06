# Cloudflare Pages Project Mapping

Quick reference for Cloudflare Pages projects and their domains.

> **Rewritten 2026-09-03.** The previous version described an old
> `projects/<name>/source/` + `deploy.zip` layout from the Allternit Websites
> repo and listed projects that no longer exist. The table below reflects the
> live projects (via `wrangler pages list`) and this repo's deploy workflows.
>
> Last verified: 2026-09-15. OSS split moved the workspace UI out of this
> repo. Cloudflare Pages Git on `ai-allternit` is **Allternit/allternit-ai**
> (`repo_id` 1370871216); merge to that repo's `main` auto-deploys production.

## Naming (do not mix these up)

| Product | Domain | GitHub source | Pages project |
|---------|--------|---------------|---------------|
| Agent workspace | **ai.allternit.com** | private **Allternit/allternit-ai** (renamed from `allternit-cloud`) | `ai-allternit` |
| Cloud console | **platform.allternit.com** | this repo `surfaces/platform.allternit.com` | `allternit-platform` |

`allternit-cloud` is not a product name. The workspace site is `ai.allternit.com`.
The cloud console is `platform.allternit.com`. `cmd/allternit-cloud-api` in this
repo is the control-plane binary for `api.allternit.com`, not the workspace UI.

**This public repo must never `wrangler pages deploy --project-name=ai-allternit`.**

Desktop release checks out `Allternit/allternit-ai` with repo secret
`ALLTERNIT_AI_CHECKOUT_TOKEN` (PAT, contents:read on that private repo).
Without it, `release-desktop.yml` cannot bundle the workspace UI.

## Active Projects

### Deployed from this repo (GitHub Actions → wrangler)

| Pages Project | Domain(s) | Source in this repo | Workflow |
|---------------|-----------|---------------------|----------|
| **allternit-platform** | platform.allternit.com | `surfaces/platform.allternit.com/dist` (Vite build) | `.github/workflows/deploy-cloudflare-pages.yml` |
| **allternit-docs** | docs.allternit.com | `surfaces/docs/dist` (unzipped `export.zip`) | `.github/workflows/deploy-docs-cloudflare.yml` |
| **allternit-office** | office.allternit.com | `surfaces/office.allternit.com/dist` | `.github/workflows/deploy-office-cloudflare.yml` |
| **allternit-office-addins** | allternit-office-addins.pages.dev | office add-in `pages-deploy/` | `.github/workflows/deploy-cloudflare-pages.yml` |
| **allternit-remote-control** | m.allternit.com | Allternit Mobile, the phone PWA (formerly fabrictransport.allternit.com; source in Allternit/allternit-ai) | wrangler CLI / allternit-ai |

### Deployed from Allternit/allternit-ai (private) — not this repo

| Pages Project | Domain(s) | Notes |
|---------------|-----------|-------|
| **ai-allternit** | ai.allternit.com | Workspace UI. Pages Git is `Allternit/allternit-ai`. Merge to `main` auto-deploys. Public-repo GHA must never wrangler this project. Optional wrangler fallback: allternit-ai `workflow_dispatch` `deploy=true`. |

### Live projects deployed outside this repo (Allternit Websites repo / manual)

| Pages Project | Domain(s) | Notes |
|---------------|-----------|-------|
| **allternit** | allternit.com, www.allternit.com | Marketing site |
| **allternit-services** | services.allternit.com | |
| **allternit-learning-labs** | labs.allternit.com | Also serves allternit-protocol-institute.pages.dev |
| **gizziio** | gizziio.com, www.gizziio.com, install.gizziio.com | |
| **gizzi-code-docs** | docs.gizziio.com | |
| **install-allternit** | install.allternit.com | |
| **try-allternit** | try.allternit.com | |
| **allternit-spaces** | spaces.allternit.com | |
| **allternit-compute** | compute.allternit.com | |
| **allternit-manufacturing** | manufacturing.allternit.com | |
| **allternit-robotics** | robotics.allternit.com | |
| **gizzi-brawl** | gizzi-brawl.pages.dev, brawl.allternit.com | Static Gizzi Brawl game; deployed from `basement-temp/public/games/kombat` |

## One project, one deploy source

**Only `surfaces/platform.allternit.com/dist` may deploy to the
`allternit-platform` Pages project** (via `deploy-cloudflare-pages.yml`,
job `deploy-platform`). The legacy `sync-platform-export.yml` workflow —
which rsynced the ai.allternit.com static export into the Allternit
Websites repo and let that repo's `deploy-platform` job push it to the
same Pages project — was retired **2026-09-11**. It caused a
last-write-wins inversion where platform.allternit.com served the AI
shell. The synced copy under
`Allternit Websites/projects/platform.allternit.com/source/` is a stale
reference snapshot and must not be deployed.

## Retired / renamed

- ~~`platform-allternit`~~ → the live project is **`allternit-platform`**.
- ~~`allternit-protocol-institute`~~ → content now served by **`allternit-learning-labs`** (labs.allternit.com).
- Do not point any domain or env var at the dead `allternit-cloud-api.fly.dev`
  host — the canonical cloud API is `https://api.allternit.com` (Contabo VPS,
  see `CLOUD_API_VPS_DEPLOY.md`).

## How to Deploy (repo-managed surfaces)

Merging to `main` deploys automatically. Manual run: GitHub → Actions → the
workflow → "Run workflow". The workflows use `wrangler pages deploy <dist>
--project-name=<project> --branch=main` with `CLOUDFLARE_API_TOKEN` and
`CLOUDFLARE_ACCOUNT_ID` secrets.

## Brand Colors

- Gizzi Mascot: `#D4B08C` (beige/tan)
- Gizzi Accents: `#D97757` (orange/coral)
- Allternit: `#B08D6E` (brown)
