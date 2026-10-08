import { copyFileSync, cpSync, existsSync, mkdirSync } from 'fs';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const dist = join(__dirname, '..', 'dist');

const routes = [
  'dashboard',
  'organizations',
  'compute',
  'agents',
  'devices',
  'runs',
  'schedules',
  'approvals',
  'models',
  'plans',
  'billing',
  'api-keys',
  'docs',
  'settings',
  'pair',
  // Platform API console pages (the 402 payment_method_required link opens platform/billing).
  'platform',
  'platform/projects',
  'platform/agents',
  'platform/conversations',
  'platform/computers',
  'platform/usage',
  'platform/billing',
  'platform/webhooks',
];

for (const route of routes) {
  const dir = join(dist, route);
  mkdirSync(dir, { recursive: true });
  copyFileSync(join(dist, 'index.html'), join(dir, 'index.html'));
}

// Clerk path-routed sign-in/sign-up navigate to sub-routes such as
// /sign-in/factor-one. Provide an index.html for the root and for the
// common sub-routes so direct loads / reloads boot the SPA with a 200.
const clerkRoutes = [
  'sign-in',
  'sign-in/factor-one',
  'sign-in/factor-two',
  'sign-up',
  'sign-up/verify-email-address',
  'sign-up/continue',
];

for (const route of clerkRoutes) {
  const dir = join(dist, route);
  mkdirSync(dir, { recursive: true });
  copyFileSync(join(dist, 'index.html'), join(dir, 'index.html'));
}

// Also provide a 404 fallback so direct links to unknown paths boot the SPA.
copyFileSync(join(dist, 'index.html'), join(dist, '404.html'));

console.log(`Copied SPA fallback to ${routes.length + clerkRoutes.length} route directories and 404.html`);

// ── Office add-in runtime ────────────────────────────────────────────────────
// The Microsoft Office task panes (Allternit for Word/Excel/PowerPoint) are
// served from this origin at /office-addins/ (see the add-in's DEPLOYMENT.md).
// Cloudflare Pages serves whatever is in dist, so the add-in build must land
// at dist/office-addins before deploy. Preferred source: the add-in's
// deploy.sh output (deployment/office-addins). CI falls back to the add-in's
// raw dist + manifests (built with production env vars in the deploy workflow).
const addinRoot = join(__dirname, '..', '..', 'allternit-extensions', 'allternit-office-addin');
const addinDeployment = join(addinRoot, 'deployment', 'office-addins');
const addinDist = join(addinRoot, 'dist');
const addinManifests = join(addinRoot, 'manifests');
const officeAddinsOut = join(dist, 'office-addins');

if (existsSync(addinDeployment)) {
  cpSync(addinDeployment, officeAddinsOut, { recursive: true });
  console.log('Embedded Office add-in runtime from deployment/office-addins at dist/office-addins');
} else if (existsSync(addinDist)) {
  cpSync(addinDist, officeAddinsOut, { recursive: true });
  if (existsSync(addinManifests)) {
    cpSync(addinManifests, join(officeAddinsOut, 'manifests'), { recursive: true });
  }
  console.log('Embedded Office add-in runtime from add-in dist/ at dist/office-addins');
} else {
  console.log('Office add-in build not found — dist/office-addins will be absent from this deployment');
}
