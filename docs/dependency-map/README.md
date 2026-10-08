# Allternit dependency map

One map of every Allternit product, the APIs and SDKs underneath, the surfaces and websites they ship on, and the code they are built from. Agents use it to see what a change can break; Eoj uses it at **admin.allternit.com**.

## Where to see it

- **admin.allternit.com** (Cloudflare Access, Eoj only). Rebuilt from `main` of both repos on every merge. Two views:
  - **Product topology**: products laid out left to right from what people use to what it is built on (apps and websites → capabilities → SDKs → APIs), plus a card per product line listing each product's type, URL, surfaces, the products it uses and the code it is built from.
  - **Features & journeys**: pick a feature to see the components it touches, or a journey to see the steps a user takes across components, numbered in order with what happens at each one. Both show the real links between those components, the shipped surfaces they reach, and the features or journeys they share. A column lists recent dated decisions across all features. `#features/<id>` links open one directly.
  - **Code graph**: pick any product, component or file path. It shows what it uses, which products it is part of, which features and journeys name it, which shipped surfaces a change reaches, and everything that could be affected, by hops.
- Locally: `bash scripts/build-admin-site.sh` with `AI_DIR=../allternit-ai`, then open `dist/admin-site/index.html`.

## Before changing a feature (agents)

1. Query impact live. It reads your working tree, so it is always current:
   `python3.11 scripts/dependency-map.py --impact allternit-ai/src/views/cowork`
   (any repo-prefixed file or folder, or a component id such as `product:factory`). Use `--ai <worktree>` when your allternit-ai checkout is not the sibling folder.
2. The output lists `changed` (the components your path belongs to), `partOf` (the products built from it), `features` and `journeys` (the ones that name it or a folder containing it), `products` and `surfaces` it can reach, and every `potentiallyAffected` component. Record the affected products, surfaces, contracts, docs and verification in your plan. Reach means review scope, not proof that every consumer needs an edit.
3. If your change adds, removes or moves a product, a runtime call (HTTP, WebSocket, spawned process), a sidecar, or a shipping path, update `products.json` or `runtime-links.json` in the same PR, with evidence paths. If it adds a feature, changes which components a feature or journey runs through, changes a feature's status, or records a product decision, update `features.json` in the same PR.
4. Before finishing, run `python3.11 scripts/dependency-map.py --validate`. It fails if any curated id or evidence path no longer resolves (for example after a rename). Fix the entry in the same PR.

There is nothing generated to commit. The admin site rebuilds itself after merge.

## How the map is made

`scripts/dependency-map.py` reads both checkouts (`git ls-files`, tracked and untracked, ignoring `.agents/`, vendored, archived and build output) and combines:

- **Packages**: every `package.json` and `Cargo.toml`, internal npm dependencies by name, Rust path dependencies including workspace inheritance and target-specific tables.
- **Feature folders**: allternit-ai `src/views`, `src/lib` and `src/components` folders; gizzi-code, allternit-api and cloud-api split until each group is under a size limit (`SPLIT` in the script). Loose Rust files group by their first name segment, so `channel_slack_app.rs` and `channel_teams_app.rs` form `allternit-api · channel_*`.
- **Code links**: static JS/TS imports (relative, `@/`, package names) and Rust `crate::name` references inside split crates.
- **Runtime links** (`runtime-links.json`, curated): the shipped surfaces (Desktop, ai.allternit.com, m.allternit.com PWA, platform.allternit.com, docs, gizzi CLI), Desktop's sidecars, and HTTP calls between the UI, allternit-api, gizzi-code, cloud-api and the voice service. Imports cannot show these, so they are recorded by hand with evidence.
- **Products** (`products.json`, curated): product lines, and per product its type (app, website, CLI, engine, service, API, SDK), URL, the surfaces it ships on, the components it is built from, and the products it uses.
- **Features and journeys** (`features.json`, curated): per feature its product, status (`live`, `building`, `planned`), description, the components it `touches` and dated `decisions` (`{"on": "YYYY-MM-DD", "note": ...}`); per journey the ordered `steps` (`{"at": component, "says": what happens}`). Both need evidence paths. The generator adds `touches` and `step` edges from them, and `has feature` edges from each product.

An arrow A → B means A depends on B. Reverse reach follows arrows backwards. "Part of" is narrower: the products whose `components` list names the component or a folder or crate that contains it.

Not inferred automatically: external package versions, Rust `use` paths outside split crates, computed dynamic imports, HTTP routes, database coupling. Record consequential ones in the curated files.

## Build and deploy

| Piece | What it does |
| --- | --- |
| Cloudflare Pages project `allternit-admin` | Git-connected to the private `Allternit/allternit-ai` repo, production branch `main`, preview deployments off, so every allternit-ai merge rebuilds it. Root directory `docs`, build command: `git clone -q --depth 1 https://github.com/Allternit/allternit-platform.git /tmp/platform && OUT=$PWD/admin-dist PAGES_ROOT=$PWD AI_DIR=$(git rev-parse --show-toplevel) bash /tmp/platform/scripts/build-admin-site.sh`, output `admin-dist`. The build writes its own `wrangler.toml` into the root directory so allternit-ai's config for ai.allternit.com is not used. No credentials: this repo is public. Custom domain admin.allternit.com. |
| `scripts/build-admin-site.sh` | Runs the generator into the output folder, copies the Access guard, adds `noindex`/`no-store` headers. |
| `surfaces/admin.allternit.com/_worker.js` | Runs in front of every request. Serves nothing unless the request carries a valid Cloudflare Access token for this application. The team domain and application AUD live in `surfaces/admin.allternit.com/access.json` (public identifiers; the signature check is what secures it). Fails closed while they are empty, and guards the `*.pages.dev` address too. Tests: `node --test surfaces/admin.allternit.com/access.test.mjs`. |
| `infrastructure/admin-site-hook/` | Worker `allternit-admin-site-hook` that receives this repo's GitHub push webhook, checks the signature, and calls the Pages deploy hook only for pushes to `main`. Tests: `node --test index.test.mjs`. |
| Cloudflare Access | Self-hosted application for `admin.allternit.com` (and `allternit-admin.pages.dev`), allowing only Eoj's email. Its AUD tag and the team domain go in `surfaces/admin.allternit.com/access.json`. |

The built site lists every file in the private allternit-ai repo. It is never committed (see `.gitignore` here) and is only served through the Access guard. This repository is public.

## Files

- `products.json`: product lines and products. Edit by hand.
- `runtime-links.json`: runtime, sidecar and shipping links. Edit by hand.
- `features.json`: features, their status and decisions, and user journeys. Edit by hand.
- `viewer.template.html`: the viewer. The generator replaces `__GRAPH_DATA__` with a compact copy of the graph (evidence capped at four files per edge).
- `../../scripts/dependency-map.py`: generator, impact query and validator.
- `../../scripts/build-admin-site.sh`: site build used by Cloudflare Pages.
