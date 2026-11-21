# Allternit Documentation

Documentation site for Allternit platform, built with [Mintlify](https://mintlify.com).

## Development

```bash
# Install Mintlify CLI
npm install -g @mintlify/cli

# Start dev server
mintlify dev
```

## Deployment

### Option 1: Mintlify Cloud (Recommended)

1. Push to GitHub
2. Connect repo at [dashboard.mintlify.com](https://dashboard.mintlify.com)
3. Auto-deploys on push to main

### Option 2: Cloudflare Pages (Current)

`.github/workflows/deploy-docs-cloudflare.yml` deploys on every push to main
that touches `surfaces/docs/`:

1. `mint export` → `dist/`
2. `node scripts/post-export.cjs dist` writes `robots.txt`, `sitemap.xml`
   (every exported page in `docs.json` navigation, trailing-slash URLs) and the
   IndexNow key file `<key>.txt` (key in `scripts/indexnow.key`). The export has
   none of these; without them Pages answers `/sitemap.xml` with the home page.
3. `wrangler pages deploy dist --project-name=allternit-docs`
4. `node scripts/indexnow.mjs docs.allternit.com` sends the sitemap URLs to
   IndexNow (Bing and the engines that share it). It never fails the deploy;
   `INDEXNOW_DRY_RUN=1` prints what it would send.

The same key and script are used by allternit-websites for the other sites.

## Structure

```
docs/
├── docs.json          # Mintlify configuration
├── introduction.mdx   # Landing page
├── quickstart.mdx     # Quick start guide
├── architecture.mdx   # Platform architecture
├── core/              # Core concepts
│   ├── gizzi-runtime.mdx
│   ├── communication.mdx
│   ├── git-dag.mdx
│   └── skills.mdx
├── byoc/              # BYOC deployment
│   ├── overview.mdx
│   ├── installation.mdx
│   ├── configuration.mdx
│   └── deployment.mdx
├── surfaces/          # User interfaces
│   ├── desktop.mdx
│   ├── platform.mdx
│   └── cli.mdx
├── api/               # API reference
│   ├── authentication.mdx
│   ├── endpoints.mdx
│   └── event-stream.mdx
└── sdk/               # SDK documentation
    ├── typescript.mdx
    └── python.mdx
```

## URLs

- Production: https://docs.allternit.com
- Preview: https://docs-allternit.pages.dev

## Contributing

1. Edit files in this directory
2. Test locally with `mintlify dev`
3. Submit PR
4. Auto-deploys on merge

## Custom Domain

Already configured in `docs.json`:
- Primary: docs.allternit.com
- Fallback: docs-allternit.pages.dev
